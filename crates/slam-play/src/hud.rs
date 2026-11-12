//! The in-game HUD: score, accuracy and combo in the skin's fonts, the skin's health bar, the
//! hit error meters, the skip button and the beatmap offset panel.
//!
//! The elements are placed by the gameplay section of the active HUD layout (ADR-0061,
//! ADR-0109): each draws in its own box with the top-left corner at (0, 0) and
//! [`LayoutElement::transform`] moves the box into place. A layout may hold several elements
//! of a kind; kinds the HUD cannot draw yet are left out.
//!
//! Draw side only. Numbers are formatted into fixed buffers and laid out again only when
//! they change; once warm ([`HudView::new`]), nothing here allocates.
//!
//! The counters and the health bar are lazer's legacy ones (`LegacyScoreCounter`,
//! `LegacyAccuracyCounter`, `LegacyDefaultComboCounter`, `LegacyHealthDisplay`) at their
//! sizes, score and accuracy rolling to new values (`RollingCounter`). With the animations
//! off ([`HudSetup::animations`]) every value shows at once. Until a skin font's digits are
//! uploaded, a counter is plain text in the UI font at this module's own size, and the health
//! a plain bar. The click-to-resume target is drawn with the cursor ([`crate::resume`]).

use std::fmt::{self, Write as _};
use std::sync::Arc;

use slam_osu::Vec2;
use slam_osu::hit_windows::HitWindows;
use slam_render::atlas::AtlasRegion;
use slam_render::sprite::{Affine, Sprite, SpriteBatch};
use slam_render::{Blend, Color};
use slam_text::{
    FamilyId, FontRegistry, GlyphCache, GlyphKey, Layout, LayoutContext, TextStats, TextStyle,
    UI_FAMILY, push_layout,
};
use slam_ui::hud_layout::{ComponentKind, LayoutElement, unit_scale};

use crate::circles::SkinSprite;
use crate::combo_counter::ComboCounter;
use crate::fps_counter::FpsLabels;
use crate::health_bar::{HealthBar, HealthTextures};
use crate::hit_error::{
    HitErrorMeter, HitErrorStyle, LAZER_PIXELS_PER_UNIT, MAX_HIT_ERROR_SPRITES, MeterTextures,
    SpinnerHiding,
};
use crate::key_overlay::{self, KEYS, KeyAnimation, KeyOverlay, KeyOverlayTextures, LABELS};
use crate::legacy_text::{FontSprites, LegacyFont};
use crate::menu_overlay::MenuLabels;
use crate::offset::{OffsetPanel, OffsetWording, format_offset};
use crate::session::{HudValues, PlaySnapshot};
use crate::song_progress::{MAX_SONG_PROGRESS_SPRITES, SongProgress};

// The sizes are in lazer's HUD pixels (1024x768, [`LAZER_PIXELS_PER_UNIT`]).
const WEIGHT: u16 = 700;
const SCORE_SIZE: f32 = 40.0;
const ACCURACY_SIZE: f32 = 24.0;
const COMBO_SIZE: f32 = 40.0;
const SKIP_SIZE: f32 = 24.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/SubmittingPlayer.cs (GameplayOffsetControl Margin)
/// The offset panel's distance from the right edge.
const OFFSET_MARGIN: f32 = 20.0;
// The panel's text size and padding are this module's own: lazer's panel holds a slider
// too.
const OFFSET_SIZE: f32 = 20.0;
const OFFSET_PADDING: f32 = 8.0;
const HEALTH_BAR: [f32; 2] = [300.0, 8.0];
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyScoreCounter.cs
const SKIN_SCORE_SCALE: f32 = 0.96;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyAccuracyCounter.cs
const SKIN_ACCURACY_SCALE: f32 = 0.6 * 0.96;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyDefaultComboCounter.cs
const SKIN_COMBO_SCALE: f32 = 1.28;
/// `font_height_ratio` and `vertical_offset` of the combo's layout: its texts scale about a
/// point this far down them.
const COMBO_FONT_HEIGHT_RATIO: f32 = 0.625;
const COMBO_VERTICAL_OFFSET: f32 = 9.0;
/// The big pop out scales a bit to the left, as in stable.
const COMBO_POP_OUT_X: f32 = 3.0;
const SKIP_BUTTON: [f32; 2] = [180.0, 48.0];

/// The most elements of a layout's gameplay section the HUD keeps; the rest are left out.
pub const MAX_HUD_ELEMENTS: usize = 32;
/// The most text elements (counters and skip buttons) the HUD draws; further ones are left
/// out, so that the text stays within [`HudView::max_glyphs`].
pub const MAX_HUD_TEXT_ELEMENTS: usize = 8;
/// The most hit error meters the HUD draws; further ones are left out.
pub const MAX_HIT_ERROR_METERS: usize = 4;
/// The most shape sprites [`HudView::push_shapes`] writes: per element the song progress's
/// most, the offset panel, the meters and the counters in skin fonts (two texts each for the
/// combo).
pub const MAX_HUD_SHAPES: usize = MAX_HUD_ELEMENTS * MAX_SONG_PROGRESS_SPRITES
    + 1
    + MAX_HIT_ERROR_METERS * MAX_HIT_ERROR_SPRITES
    + MAX_HUD_TEXT_ELEMENTS * 2 * NUMBER_TEXT_LEN;

/// The property of a hit error meter element with its style: `"legacy"` (the default) or
/// `"bar"` (ADR-0050).
pub const METER_STYLE_PROPERTY: &str = "style";

const TEXT: Color = Color::WHITE;
const SHADE: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.5,
};

/// Every character the numbers use, as long as the longest number text: laying it out
/// once settles the layout buffers, and its glyphs are the ones to warm.
const WARM_TEXT: &str = "0123456789.%x-0123456789";
/// The numbers without a letter: a shape plan is made per script, and digits alone have
/// none (the combo's `x` is Latin).
const WARM_DIGITS: &str = "0123456789.%-";
/// The key overlay's captions and counts.
const WARM_KEYS: &str = "K1K2M1M2 4294967295";
/// The key overlay's text size in lazer's HUD pixels: McOsu's 0.3 of the key's height (46).
const KEY_TEXT_SIZE: f32 = 13.8;

/// Stands for the number in the localised texts around one: the offset panel's
/// ([`HudLabels`]) and the menus' ([`crate::menu_overlay::MenuLabels`]).
pub const OFFSET_PLACEHOLDER: &str = "\u{FFFC}";
/// What the offset's digits are laid out with to warm the panel's texts.
const WARM_OFFSET: &str = "-0123456789.";

/// The HUD's localised texts.
#[derive(Debug, Clone)]
pub struct HudLabels {
    /// The skip button's text.
    pub skip: String,
    /// The offset panel's text at zero, with [`OFFSET_PLACEHOLDER`] for the number.
    pub offset_zero: String,
    /// The panel's text for a positive offset: objects appear earlier.
    pub offset_earlier: String,
    /// The panel's text for a negative offset: objects appear later.
    pub offset_later: String,
    /// The pause and fail menus' texts.
    pub menu: MenuLabels,
    /// The FPS counter's texts.
    pub fps: FpsLabels,
}

/// A localised text around a number.
#[derive(Debug, Clone)]
pub(crate) struct Template {
    pub(crate) prefix: String,
    pub(crate) suffix: String,
}

impl Template {
    /// `label` split at [`OFFSET_PLACEHOLDER`]; without one, the number is left out.
    pub(crate) fn new(label: &str) -> Template {
        let (prefix, suffix) = label.split_once(OFFSET_PLACEHOLDER).unwrap_or((label, ""));
        Template {
            prefix: prefix.to_owned(),
            suffix: suffix.to_owned(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.prefix.len() + self.suffix.len()
    }

    /// Writes the text with `number` into `out`. Allocation-free once `out` has room.
    pub(crate) fn write(&self, number: &str, out: &mut String) {
        out.clear();
        out.push_str(&self.prefix);
        out.push_str(number);
        out.push_str(&self.suffix);
    }
}

/// Bytes of a number's text.
pub(crate) const NUMBER_TEXT_LEN: usize = WARM_TEXT.len();

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

impl Default for NumberText {
    fn default() -> Self {
        Self::new()
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
pub(crate) fn number_text(args: fmt::Arguments<'_>) -> NumberText {
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

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyScoreCounter.cs (RollingDuration, RollingEasing)
const SCORE_ROLLING_DURATION: f64 = 1000.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/PercentageCounter.cs (RollingDuration)
const ACCURACY_ROLLING_DURATION: f64 = 375.0;

/// A counter's shown value rolling to the latest one (`RollingCounter.TransformCount`): from
/// what it shows, over a fixed time, `Easing.Out` (quadratic).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/RollingCounter.cs (TransformCount)
#[derive(Debug, Clone, Copy, PartialEq)]
struct Roll {
    from: f64,
    to: f64,
    start: f64,
    duration: f64,
}

impl Roll {
    /// Showing `value` already: a new counter, as lazer's rolls from its own value to it
    /// on load without moving.
    fn at_rest(value: f64) -> Roll {
        Roll {
            from: value,
            to: value,
            start: f64::NEG_INFINITY,
            duration: 0.0,
        }
    }

    fn at(&self, time: f64) -> f64 {
        if self.duration <= 0.0 || time >= self.start + self.duration {
            return self.to;
        }
        let t = ((time - self.start) / self.duration).max(0.0);
        self.from + (self.to - self.from) * (t * (2.0 - t))
    }

    /// Rolls to `value` from `start` on, if it is a new one, from what showed at `shown`
    /// (the frame before: lazer's `DisplayedCount`, a whole number for the score).
    fn set(&mut self, value: f64, shown: f64, start: f64, duration: f64, whole: bool) {
        if value != self.to {
            let from = self.at(shown);
            *self = Roll {
                from: if whole { from.round_ties_even() } else { from },
                to: value,
                start,
                duration,
            };
        }
    }
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
    /// Window pixels per lazer HUD pixel: the sizes' unit.
    scale: f32,
}

/// What a layout element draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Score,
    Accuracy,
    Combo,
    Health,
    SongProgress,
    KeyCounter,
    SkipButton,
    /// The meter at this index of [`HudView::meters`].
    Meter(usize),
}

/// A layout element the HUD draws.
#[derive(Debug, Clone)]
struct Placed {
    part: Part,
    element: LayoutElement,
    /// The index in [`HudView::elements`] of the element it is attached to (ADR-0112).
    attached_to: Option<usize>,
}

/// The style a hit error meter element asks for; unknown values give the default.
fn meter_style(element: &LayoutElement) -> HitErrorStyle {
    match element
        .properties
        .get(METER_STYLE_PROPERTY)
        .and_then(|v| v.as_str())
    {
        Some("bar") => HitErrorStyle::Bar,
        _ => HitErrorStyle::Legacy,
    }
}

/// The HUD's skin textures for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HudTextures {
    /// The score font (`ScorePrefix`), for score and accuracy.
    pub score: FontSprites,
    /// The combo font (`ComboPrefix`).
    pub combo: FontSprites,
    pub health: HealthTextures,
    /// A white disc and the song progress's ring ([`crate::song_progress::ring_image`]).
    pub disc: Option<AtlasRegion>,
    pub ring: Option<AtlasRegion>,
    /// The key overlay's textures and text colour (`InputOverlayText`; black without).
    pub keys: KeyOverlayTextures,
    pub key_text: Option<Color>,
    /// The skip button's frames (`play-skip`).
    pub skip: SkipTextures,
}

/// The most frames of the skip button drawn.
pub const MAX_SKIP_FRAMES: usize = 32;

/// The skip button's frames, drawn once all are uploaded; none: the plain button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkipTextures {
    pub frames: [Option<SkinSprite>; MAX_SKIP_FRAMES],
    pub count: usize,
    /// How long each frame shows, in milliseconds.
    pub frame_length: f64,
}

impl Default for SkipTextures {
    fn default() -> Self {
        SkipTextures {
            frames: [None; MAX_SKIP_FRAMES],
            count: 0,
            frame_length: 1000.0 / 60.0,
        }
    }
}

impl SkipTextures {
    /// The frame at `time` (milliseconds from any fixed point), if there are frames.
    fn frame(&self, time: f64) -> Option<SkinSprite> {
        if self.count == 0 {
            return None;
        }
        let index = if self.frame_length > 0.0 {
            (time.max(0.0) / self.frame_length) as usize % self.count
        } else {
            0
        };
        self.frames[index]
    }
}

impl HudTextures {
    /// The font `part` draws in, if it is a counter and the font's textures are loaded.
    fn font(&self, part: Part) -> Option<LegacyFont<'_>> {
        let (sprites, fixed) = match part {
            Part::Score | Part::Accuracy => (&self.score, true),
            Part::Combo => (&self.combo, false),
            _ => return None,
        };
        sprites.loaded.then(|| sprites.font(fixed))
    }
}

/// What [`HudView::new`] needs of the play beside the texts.
#[derive(Debug, Clone, Copy)]
pub struct HudSetup<'a> {
    /// The gameplay section of the active HUD layout.
    pub layout: &'a [LayoutElement],
    /// The hit windows of the beatmap with the mods applied, for the hit error meters.
    pub windows: &'a HitWindows,
    /// When the hit error meters hide for spinners.
    pub meter_hiding: &'a SpinnerHiding,
    /// The times of the song progress.
    pub song_progress: SongProgress,
    /// The key overlay's animation; `None`: the overlay is off and its elements are left
    /// out (ADR-0113).
    pub key_overlay: Option<KeyAnimation>,
    /// Whether the counters roll, the combo pops and the health bar eases, bulges and
    /// flashes; off, they show their values at once.
    pub animations: bool,
}

/// The draw side of the stand-in HUD.
pub struct HudView {
    registry: Arc<FontRegistry>,
    family: FamilyId,
    context: LayoutContext,
    frame: Frame,
    /// The layout's elements the HUD draws, in the layout's order (later on top).
    elements: Vec<Placed>,
    meters: Vec<HitErrorMeter>,
    meter_hiding: SpinnerHiding,
    song_progress: SongProgress,
    animations: bool,
    /// The values last taken, and the score and accuracy as they roll to them.
    values: HudValues,
    /// `None` until the first values: the counters start at them.
    score_roll: Option<Roll>,
    accuracy_roll: Option<Roll>,
    key_overlay: KeyOverlay,
    /// The keys' captions or counts, laid out when they change.
    key_texts: [Counter; KEYS],
    /// The combo counter's animation; `None` until the first frame.
    combo_counter: Option<ComboCounter>,
    health_bar: HealthBar,
    /// The attempt the animations are of ([`PlaySnapshot::restarts`]).
    restarts: u32,
    /// The HUD's clock at the last [`HudView::animate`], in milliseconds.
    time: f64,
    score: Counter,
    accuracy: Counter,
    combo: Counter,
    skip_label: String,
    skip: Layout,
    /// The offset panel's texts: at zero, earlier, later ([`OffsetWording`]).
    offset_templates: [Template; 3],
    /// The offset the panel's layout shows, as bits; `None` lays it out on the next update.
    offset_shown: Option<u64>,
    offset_text: String,
    offset: Layout,
    /// [`WARM_TEXT`] and [`WARM_DIGITS`] laid out: the glyphs to warm (each script may
    /// shape to other glyphs).
    warm: [Layout; 3],
    /// The offset panel's texts laid out with [`WARM_OFFSET`].
    warm_offset: [Layout; 3],
}

impl std::fmt::Debug for HudView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HudView")
            .field("frame", &self.frame)
            .field("elements", &self.elements)
            .field("score", &self.score.text)
            .field("accuracy", &self.accuracy.text)
            .field("combo", &self.combo.text)
            .field("skip_label", &self.skip_label)
            .finish_non_exhaustive()
    }
}

impl HudView {
    /// A HUD with fonts from `registry`, the localised `labels` and the elements of
    /// `setup`'s layout. Lays out every character it will show, so that later layouts do
    /// not allocate: call it off the hot paths.
    pub fn new(registry: Arc<FontRegistry>, labels: &HudLabels, setup: HudSetup<'_>) -> HudView {
        let mut elements = Vec::new();
        let mut meters = Vec::new();
        let mut texts = 0;
        for element in setup.layout {
            if elements.len() == MAX_HUD_ELEMENTS {
                tracing::warn!(
                    "HUD layout has more than {MAX_HUD_ELEMENTS} elements; the rest are left out"
                );
                break;
            }
            if element.component == ComponentKind::KeyCounter && setup.key_overlay.is_none() {
                continue;
            }
            let text = matches!(
                element.component,
                ComponentKind::Score
                    | ComponentKind::Accuracy
                    | ComponentKind::Combo
                    | ComponentKind::KeyCounter
                    | ComponentKind::SkipButton
            );
            if text && texts == MAX_HUD_TEXT_ELEMENTS {
                tracing::warn!(
                    "HUD layout has more than {MAX_HUD_TEXT_ELEMENTS} text elements; the rest are left out"
                );
                continue;
            }
            texts += usize::from(text);
            let part = match element.component {
                ComponentKind::Score => Part::Score,
                ComponentKind::Accuracy => Part::Accuracy,
                ComponentKind::Combo => Part::Combo,
                ComponentKind::Health => Part::Health,
                ComponentKind::SongProgress => Part::SongProgress,
                ComponentKind::KeyCounter => Part::KeyCounter,
                ComponentKind::SkipButton => Part::SkipButton,
                ComponentKind::HitErrorMeter if meters.len() < MAX_HIT_ERROR_METERS => {
                    meters.push(HitErrorMeter::new(meter_style(element), setup.windows));
                    Part::Meter(meters.len() - 1)
                }
                ComponentKind::HitErrorMeter => {
                    tracing::warn!(
                        "HUD layout has more than {MAX_HIT_ERROR_METERS} hit error meters; the rest are left out"
                    );
                    continue;
                }
                // Not drawn yet.
                _ => continue,
            };
            // The first element of the kind before it (ADR-0112); none: the screen.
            let attached_to = element.attach.as_ref().and_then(|attach| {
                elements
                    .iter()
                    .position(|p: &Placed| p.element.component == attach.to)
            });
            elements.push(Placed {
                part,
                element: element.clone(),
                attached_to,
            });
        }
        let offset_templates = [
            Template::new(&labels.offset_zero),
            Template::new(&labels.offset_earlier),
            Template::new(&labels.offset_later),
        ];
        let longest = offset_templates
            .iter()
            .map(Template::len)
            .max()
            .unwrap_or(0);
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
            elements,
            meters,
            meter_hiding: setup.meter_hiding.clone(),
            song_progress: setup.song_progress,
            animations: setup.animations,
            values: HudValues::default(),
            score_roll: None,
            accuracy_roll: None,
            key_overlay: KeyOverlay::new(setup.key_overlay.unwrap_or_default()),
            key_texts: std::array::from_fn(|_| Counter::new(KEY_TEXT_SIZE)),
            combo_counter: None,
            health_bar: HealthBar::new(setup.animations),
            restarts: 0,
            time: 0.0,
            score: Counter::new(SCORE_SIZE),
            accuracy: Counter::new(ACCURACY_SIZE),
            combo: Counter::new(COMBO_SIZE),
            skip_label: labels.skip.clone(),
            skip: Layout::new(),
            offset_templates,
            offset_shown: None,
            offset_text: String::with_capacity(longest + NUMBER_TEXT_LEN),
            offset: Layout::new(),
            warm: [Layout::new(), Layout::new(), Layout::new()],
            warm_offset: [Layout::new(), Layout::new(), Layout::new()],
        };
        let style = hud.style(1.0);
        for counter in [&mut hud.score, &mut hud.accuracy, &mut hud.combo] {
            hud.context
                .layout_into(&hud.registry, WARM_DIGITS, &style, &mut counter.layout);
            hud.context
                .layout_into(&hud.registry, WARM_TEXT, &style, &mut counter.layout);
        }
        for counter in &mut hud.key_texts {
            hud.context
                .layout_into(&hud.registry, WARM_KEYS, &style, &mut counter.layout);
        }
        let [text, digits, keys] = &mut hud.warm;
        hud.context
            .layout_into(&hud.registry, WARM_KEYS, &style, keys);
        hud.context
            .layout_into(&hud.registry, WARM_TEXT, &style, text);
        hud.context
            .layout_into(&hud.registry, WARM_DIGITS, &style, digits);
        hud.context
            .layout_into(&hud.registry, &hud.skip_label, &style, &mut hud.skip);
        for (template, warm) in hud.offset_templates.iter().zip(&mut hud.warm_offset) {
            template.write(WARM_OFFSET, &mut hud.offset_text);
            hud.context
                .layout_into(&hud.registry, &hud.offset_text, &style, warm);
            hud.context
                .layout_into(&hud.registry, &hud.offset_text, &style, &mut hud.offset);
        }
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
            .chain(&self.warm_offset)
            .flat_map(Layout::glyphs)
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), slam_text::Glyph::Pending))
            .count()
    }

    /// Lays out what changed for the HUD `values` and the beatmap offset `offset_ms` on a
    /// window of `width` × `height` pixels. Allocation-free.
    pub fn update(&mut self, width: f32, height: f32, values: &HudValues, offset_ms: f64) {
        let frame = Frame {
            width,
            height,
            scale: unit_scale(width, height) / LAZER_PIXELS_PER_UNIT,
        };
        if frame != self.frame {
            self.frame = frame;
            for counter in [&mut self.score, &mut self.accuracy, &mut self.combo]
                .into_iter()
                .chain(&mut self.key_texts)
            {
                counter.shown = None;
            }
            self.offset_shown = None;
            let style = self.style(SKIP_SIZE * frame.scale);
            self.context
                .layout_into(&self.registry, &self.skip_label, &style, &mut self.skip);
        }
        self.values = *values;
        self.lay_out_counters();
        let scale = self.frame.scale;
        if self.offset_shown != Some(offset_ms.to_bits()) {
            self.offset_shown = Some(offset_ms.to_bits());
            let mut digits = NumberText::new();
            // The digits of an offset in range always fit.
            let wording = format_offset(offset_ms, &mut digits).unwrap_or(OffsetWording::Zero);
            let template = &self.offset_templates[match wording {
                OffsetWording::Zero => 0,
                OffsetWording::Earlier => 1,
                OffsetWording::Later => 2,
            }];
            template.write(digits.as_str(), &mut self.offset_text);
            let style = self.style(OFFSET_SIZE * scale);
            self.context
                .layout_into(&self.registry, &self.offset_text, &style, &mut self.offset);
        }
    }

    /// The score and accuracy shown now: rolling, or the values themselves.
    fn shown_values(&self) -> (i64, f64) {
        if !self.animations {
            return (self.values.score, self.values.accuracy);
        }
        // `Interpolation.ValueAt` of a `long`: `Math.Round`, halves to even.
        let score = self.score_roll.map_or(self.values.score, |r| {
            r.at(self.time).round_ties_even() as i64
        });
        let accuracy = self
            .accuracy_roll
            .map_or(self.values.accuracy, |r| r.at(self.time));
        (score, accuracy)
    }

    /// Lays out the counters' texts that changed. Allocation-free.
    fn lay_out_counters(&mut self) {
        let (score, accuracy) = self.shown_values();
        let combo = self.values.combo;
        // Hundredths of a percent as shown, so that a change of a smaller digit does not
        // lay out the same text again.
        let accuracy = format_accuracy(accuracy);
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
        set(&mut self.score, score.max(0) as u64, format_score(score));
        set(&mut self.accuracy, accuracy_key, accuracy);
        set(&mut self.combo, u64::from(combo), format_combo(combo));
    }

    /// Moves the animations to engine time `now_ns`: the hit error meters take the new
    /// judgements of `snapshot` and drop what expired, the combo counter and the health bar
    /// take its values. A restart starts them over. Call once a frame. Allocation-free.
    pub fn animate(&mut self, snapshot: &PlaySnapshot, now_ns: u64) {
        let time = snapshot.time_at(now_ns);
        for meter in &mut self.meters {
            meter.update(&snapshot.hit_errors, snapshot.restarts, time);
        }
        // The HUD is not on the gameplay clock.
        let shown = self.time;
        self.time = now_ns as f64 / 1e6;
        if snapshot.restarts != self.restarts {
            self.restarts = snapshot.restarts;
            self.combo_counter = None;
            self.health_bar = HealthBar::new(self.animations);
            // A new attempt's counters start at its values.
            self.score_roll = None;
            self.accuracy_roll = None;
        }
        let combo = snapshot.hud.combo;
        match &mut self.combo_counter {
            Some(counter) => counter.update(combo, self.time),
            None => self.combo_counter = Some(ComboCounter::new(combo, self.time)),
        }
        self.health_bar
            .update(snapshot.health, snapshot.hud.hits, self.time);
        self.key_overlay.update(&snapshot.keys, self.time);
        // `TransformCount` at the frame's time, from what the last frame showed.
        let time = self.time;
        let roll = |roll: &mut Option<Roll>, value: f64, duration: f64, whole: bool| match roll {
            Some(roll) => roll.set(value, shown, time, duration, whole),
            None => *roll = Some(Roll::at_rest(value)),
        };
        roll(
            &mut self.score_roll,
            snapshot.hud.score as f64,
            SCORE_ROLLING_DURATION,
            true,
        );
        roll(
            &mut self.accuracy_roll,
            snapshot.hud.accuracy,
            ACCURACY_ROLLING_DURATION,
            false,
        );
        self.lay_out_counters();
        // Captions until the first press, then the counts (McOsu `drawInputOverlay`).
        let size = KEY_TEXT_SIZE * self.frame.scale;
        for (i, (counter, &count)) in self
            .key_texts
            .iter_mut()
            .zip(&self.key_overlay.counts())
            .enumerate()
        {
            let key = u64::from(count);
            if counter.shown == Some(key) {
                continue;
            }
            counter.shown = Some(key);
            counter.text = if count == 0 {
                number_text(format_args!("{}", LABELS[i]))
            } else {
                number_text(format_args!("{count}"))
            };
            let style = TextStyle::new(self.family, WEIGHT, size);
            self.context.layout_into(
                &self.registry,
                counter.text.as_str(),
                &style,
                &mut counter.layout,
            );
        }
    }

    /// The most glyph sprites [`HudView::push_text`] writes: each text element's longest text
    /// and the offset panel's. A text batch with this room does not allocate.
    pub fn max_glyphs(&self) -> usize {
        let counter = self
            .warm
            .iter()
            .map(|l| l.glyphs().len())
            .max()
            .unwrap_or(0);
        let offset = self
            .warm_offset
            .iter()
            .map(|l| l.glyphs().len())
            .max()
            .unwrap_or(0);
        let elements: usize = self
            .elements
            .iter()
            .map(|placed| match placed.part {
                Part::Score | Part::Accuracy | Part::Combo => counter,
                Part::SkipButton => self.skip.glyphs().len(),
                Part::KeyCounter => KEYS * NUMBER_TEXT_LEN,
                Part::Health | Part::SongProgress | Part::Meter(_) => 0,
            })
            .sum();
        elements + offset
    }

    /// The hit error meters, in the layout's order.
    pub fn meters(&self) -> &[HitErrorMeter] {
        &self.meters
    }

    /// The combo counter's texts at the last [`HudView::animate`]: its own and the popped-out
    /// one, with the pose.
    fn combo_pose(&self) -> Option<crate::combo_counter::ComboPose> {
        if !self.animations {
            return Some(crate::combo_counter::ComboPose {
                shown: self.values.combo,
                shown_alpha: 1.0,
                shown_scale: 1.0,
                pop_out: None,
            });
        }
        self.combo_counter.as_ref().map(|c| c.pose(self.time))
    }

    /// The box of `part` in window pixels as last laid out, drawn with `textures`.
    fn part_size(&self, part: Part, textures: &HudTextures) -> [f32; 2] {
        let scale = self.frame.scale;
        let text = |layout: &Layout| [layout.width(), layout.height()];
        if let Some(font) = textures.font(part) {
            let (size, counter_scale) = match part {
                Part::Score => (
                    font.measure(self.score.text.as_str().as_bytes()),
                    SKIN_SCORE_SCALE,
                ),
                Part::Accuracy => (
                    font.measure(self.accuracy.text.as_str().as_bytes()),
                    SKIN_ACCURACY_SCALE,
                ),
                // The counter is as big as its own text (`counterContainer.Size`).
                _ => {
                    let shown = self.combo_pose().map_or(0, |p| p.shown);
                    (
                        font.measure(format_combo(shown).as_str().as_bytes()),
                        SKIN_COMBO_SCALE,
                    )
                }
            };
            let s = counter_scale * scale;
            return [size.x * s, size.y * s];
        }
        if part == Part::SkipButton
            && let Some(frame) = textures.skip.frame(0.0)
        {
            return [frame.size.x * scale, frame.size.y * scale];
        }
        if part == Part::Health && textures.health.is_drawn() {
            let [w, h] = textures.health.size();
            return [w * scale, h * scale];
        }
        match part {
            Part::Score => text(&self.score.layout),
            Part::Accuracy => text(&self.accuracy.layout),
            Part::Combo => text(&self.combo.layout),
            Part::Health => [HEALTH_BAR[0] * scale, HEALTH_BAR[1] * scale],
            Part::SongProgress => {
                let side = crate::song_progress::SIZE * scale;
                [side, side]
            }
            Part::KeyCounter => {
                let [w, h] = key_overlay::size(&textures.keys);
                [w * scale, h * scale]
            }
            Part::SkipButton => [SKIP_BUTTON[0] * scale, SKIP_BUTTON[1] * scale],
            Part::Meter(i) => self.meters[i].size(scale),
        }
    }

    /// The elements to draw for `snapshot` with `textures`, with their map into the window
    /// and alpha. An attached element is placed from its target's box (ADR-0112), hidden
    /// or not.
    fn placed<'a>(
        &'a self,
        snapshot: &'a PlaySnapshot,
        textures: &'a HudTextures,
    ) -> impl Iterator<Item = (Part, Affine, f32)> + 'a {
        let Frame { width, height, .. } = self.frame;
        let mut boxes = [(Affine::IDENTITY, [0.0f32; 2]); MAX_HUD_ELEMENTS];
        for (i, placed) in self.elements.iter().enumerate() {
            let element = &placed.element;
            let size = self.part_size(placed.part, textures);
            let map = match (placed.attached_to, &element.attach) {
                (Some(target), Some(attach)) => {
                    let (target_map, target_size) = boxes[target];
                    let [fx, fy] = attach.point.fraction();
                    let point = target_map.apply([fx * target_size[0], fy * target_size[1]]);
                    element.transform_from(point, width, height, size)
                }
                _ => element.transform(width, height, size),
            };
            boxes[i] = (map, size);
        }
        self.elements
            .iter()
            .zip(boxes)
            .filter_map(move |(placed, (map, _))| {
                let element = &placed.element;
                let shown = element.visible
                    && element.alpha > 0.0
                    && (placed.part != Part::SkipButton || snapshot.skip_available);
                shown.then_some((placed.part, map, element.alpha.min(1.0)))
            })
    }

    /// The offset panel's text as last laid out.
    pub fn offset_text(&self) -> &str {
        &self.offset_text
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayOffsetControl.cs
    /// The offset panel's box (position and size) and alpha at `now_ns`, if it shows: at
    /// the right edge, centred vertically, moving right as it fades.
    fn offset_box(&self, panel: &OffsetPanel, now_ns: u64) -> Option<([f32; 2], [f32; 2], f32)> {
        let alpha = panel.alpha(now_ns);
        if alpha <= 0.0 {
            return None;
        }
        let Frame {
            width,
            height,
            scale,
        } = self.frame;
        let padding = OFFSET_PADDING * scale;
        let size = [
            self.offset.width() + 2.0 * padding,
            self.offset.height() + 2.0 * padding,
        ];
        let x = width - OFFSET_MARGIN * scale - size[0] + panel.shift(now_ns) * scale;
        Some(([x, (height - size[1]) / 2.0], size, alpha))
    }

    /// The texts of the counters as last laid out: score, accuracy, combo.
    pub fn texts(&self) -> [&str; 3] {
        [
            self.score.text.as_str(),
            self.accuracy.text.as_str(),
            self.combo.text.as_str(),
        ]
    }

    /// Writes the HUD's shapes for `snapshot` at engine time `now_ns`: the counters in skin
    /// fonts and the health bar with `textures`, plain health bars, the skip buttons and the
    /// offset panel tinting `white` (an opaque white image), the hit error meters with
    /// `meter`'s images, faded during spinners. Allocation-free once `batch` has room for
    /// [`MAX_HUD_SHAPES`].
    pub fn push_shapes(
        &self,
        batch: &mut SpriteBatch,
        snapshot: &PlaySnapshot,
        now_ns: u64,
        white: Option<AtlasRegion>,
        meter: Option<&MeterTextures>,
        textures: &HudTextures,
    ) {
        let scale = self.frame.scale;
        let time = snapshot.time_at(now_ns);
        let meter_alpha = self.meter_hiding.alpha(time);
        let rect = |batch: &mut SpriteBatch, white: AtlasRegion, position, size, colour| {
            let mut sprite = Sprite::new(white.texture, white.uv, position, size);
            sprite.colour = colour;
            batch.push(&sprite);
        };
        for (part, map, alpha) in self.placed(snapshot, textures) {
            let first = batch.len();
            let mut alpha = alpha;
            if let Some(font) = textures.font(part) {
                self.push_counter(batch, part, &font);
                batch.transform_from(first, &map, [1.0, 1.0, 1.0, alpha]);
                continue;
            }
            match (part, white) {
                (Part::KeyCounter, _) => {
                    self.key_overlay
                        .push(batch, &textures.keys, scale, self.time);
                }
                (Part::SkipButton, _) if textures.skip.count > 0 => {
                    let Some(frame) = textures.skip.frame(self.time) else {
                        continue;
                    };
                    batch.push(&Sprite::new(
                        frame.texture,
                        frame.uv,
                        [0.0, 0.0],
                        [frame.size.x * scale, frame.size.y * scale],
                    ));
                }
                (Part::SongProgress, _) => {
                    let (Some(disc), Some(ring)) = (textures.disc, textures.ring) else {
                        continue;
                    };
                    self.song_progress.push(batch, time, scale, disc, ring);
                }
                (Part::Health, _) if textures.health.is_drawn() => {
                    self.health_bar
                        .push(batch, &textures.health, scale, self.time);
                }
                (Part::Health, Some(white)) => {
                    let bar = self.part_size(part, textures);
                    rect(batch, white, [0.0, 0.0], bar, SHADE);
                    let health = snapshot.health.clamp(0.0, 1.0) as f32;
                    rect(batch, white, [0.0, 0.0], [bar[0] * health, bar[1]], TEXT);
                }
                (Part::SkipButton, Some(white)) => {
                    rect(
                        batch,
                        white,
                        [0.0, 0.0],
                        self.part_size(part, textures),
                        SHADE,
                    );
                }
                (Part::Meter(i), _) => {
                    let Some(textures) = meter else { continue };
                    alpha *= meter_alpha;
                    if alpha <= 0.0 {
                        continue;
                    }
                    self.meters[i].push(batch, scale, time, textures);
                }
                _ => continue,
            }
            batch.transform_from(first, &map, [1.0, 1.0, 1.0, alpha]);
        }
        if let Some(white) = white
            && let Some((position, size, alpha)) = self.offset_box(&snapshot.offset_panel, now_ns)
        {
            let shade = Color {
                a: SHADE.a * alpha,
                ..SHADE
            };
            rect(batch, white, position, size, shade);
        }
    }

    /// Writes the counter `part` in `font` in its box, top left at (0, 0).
    fn push_counter(&self, batch: &mut SpriteBatch, part: Part, font: &LegacyFont<'_>) {
        let scale = self.frame.scale;
        let top_left = Vec2::ZERO;
        let at_top_left =
            |batch: &mut SpriteBatch, text: &str, at: Vec2, size: f32, colour, blend| {
                font.push(batch, text.as_bytes(), at, top_left, size, colour, blend);
            };
        match part {
            Part::Score => at_top_left(
                batch,
                self.score.text.as_str(),
                top_left,
                SKIN_SCORE_SCALE * scale,
                TEXT,
                Blend::Alpha,
            ),
            Part::Accuracy => at_top_left(
                batch,
                self.accuracy.text.as_str(),
                top_left,
                SKIN_ACCURACY_SCALE * scale,
                TEXT,
                Blend::Alpha,
            ),
            Part::Combo => {
                let Some(pose) = self.combo_pose() else {
                    return;
                };
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyDefaultComboCounter.cs (updateLayout)
                // In the counter's own units: each text scales about a point
                // `COMBO_FONT_HEIGHT_RATIO` down it (plus the offset), placed so that at a
                // scale of 1 the counter's text fills the box and the pop out's bottom meets
                // it, 3 to the left.
                let unit = SKIN_COMBO_SCALE * scale;
                let shown = format_combo(pose.shown);
                let height = font.measure(shown.as_str().as_bytes()).y;
                let pivot = |h: f32| COMBO_FONT_HEIGHT_RATIO * h + COMBO_VERTICAL_OFFSET;
                if let Some((value, pop_scale, pop_alpha)) = pose.pop_out {
                    let text = format_combo(value);
                    let h = font.measure(text.as_str().as_bytes()).y;
                    let anchor = Vec2::new(
                        0.0,
                        height - (1.0 - COMBO_FONT_HEIGHT_RATIO) * h + COMBO_VERTICAL_OFFSET,
                    );
                    let corner = Vec2::new(
                        anchor.x - COMBO_POP_OUT_X * pop_scale,
                        anchor.y - pivot(h) * pop_scale,
                    );
                    at_top_left(
                        batch,
                        text.as_str(),
                        Vec2::new(corner.x * unit, corner.y * unit),
                        pop_scale * unit,
                        Color {
                            a: pop_alpha,
                            ..TEXT
                        },
                        Blend::Additive,
                    );
                }
                let corner_y = pivot(height) * (1.0 - pose.shown_scale);
                at_top_left(
                    batch,
                    shown.as_str(),
                    Vec2::new(0.0, corner_y * unit),
                    pose.shown_scale * unit,
                    Color {
                        a: pose.shown_alpha,
                        ..TEXT
                    },
                    Blend::Alpha,
                );
            }
            _ => {}
        }
    }

    /// Writes the HUD's text for `snapshot` into a batch drawn with the MSDF shader: what is not
    /// in a skin font with `textures`.
    /// Glyphs not ready yet are skipped and counted. Allocation-free once `batch` has
    /// room.
    pub fn push_text(
        &self,
        batch: &mut SpriteBatch,
        snapshot: &PlaySnapshot,
        now_ns: u64,
        glyphs: &mut GlyphCache,
        textures: &HudTextures,
    ) -> TextStats {
        let scale = self.frame.scale;
        let mut stats = TextStats::default();
        let mut text = |batch: &mut SpriteBatch, layout: &Layout, position: [f32; 2], colour| {
            let s = push_layout(batch, layout, position, colour, glyphs);
            stats.drawn += s.drawn;
            stats.pending += s.pending;
        };
        for (part, map, alpha) in self.placed(snapshot, textures) {
            if textures.font(part).is_some() {
                continue;
            }
            let first = batch.len();
            match part {
                Part::Score => text(batch, &self.score.layout, [0.0, 0.0], TEXT),
                Part::Accuracy => text(batch, &self.accuracy.layout, [0.0, 0.0], TEXT),
                Part::Combo => text(batch, &self.combo.layout, [0.0, 0.0], TEXT),
                // No captions without the keys under them.
                Part::KeyCounter if textures.keys.key.is_none() => continue,
                Part::KeyCounter => {
                    let [width, _] = key_overlay::size(&textures.keys);
                    for (i, counter) in self.key_texts.iter().enumerate() {
                        let key_first = batch.len();
                        let [cx, cy] = key_overlay::key_centre(width, i);
                        let centre = [cx * scale, cy * scale];
                        let layout = &counter.layout;
                        let at = [
                            centre[0] - layout.width() / 2.0,
                            centre[1] - layout.height() / 2.0,
                        ];
                        text(batch, layout, at, textures.key_text.unwrap_or(Color::BLACK));
                        // The text shrinks with its key.
                        let pose = self.key_overlay.pose(i, self.time);
                        batch.transform_from(
                            key_first,
                            &Affine::about(centre, pose.scale, 0.0, [0.0, 0.0]),
                            [1.0; 4],
                        );
                    }
                }
                Part::SkipButton if textures.skip.count > 0 => continue,
                Part::SkipButton => {
                    let [w, h] = self.part_size(part, textures);
                    let skip = &self.skip;
                    let at = [(w - skip.width()) / 2.0, (h - skip.height()) / 2.0];
                    text(batch, skip, at, TEXT);
                }
                Part::Health | Part::SongProgress | Part::Meter(_) => continue,
            }
            batch.transform_from(first, &map, [1.0, 1.0, 1.0, alpha]);
        }
        if let Some((position, _, alpha)) = self.offset_box(&snapshot.offset_panel, now_ns) {
            let padding = OFFSET_PADDING * scale;
            text(
                batch,
                &self.offset,
                [position[0] + padding, position[1] + padding],
                Color { a: alpha, ..TEXT },
            );
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slam_ui::hud_layout::Anchor;

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

    fn labels() -> HudLabels {
        HudLabels {
            skip: "Skip".to_owned(),
            offset_zero: format!("{OFFSET_PLACEHOLDER} ms"),
            offset_earlier: format!("{OFFSET_PLACEHOLDER} ms (hit objects appear earlier)"),
            offset_later: format!("{OFFSET_PLACEHOLDER} ms (hit objects appear later)"),
            menu: crate::menu_overlay::tests::labels(),
            fps: crate::fps_counter::tests::labels(),
        }
    }

    fn hud(layout: &[LayoutElement]) -> HudView {
        hud_with(layout, true)
    }

    fn hud_with(layout: &[LayoutElement], animations: bool) -> HudView {
        HudView::new(
            Arc::new(FontRegistry::with_builtin()),
            &labels(),
            HudSetup {
                layout,
                windows: &HitWindows::new(5.0),
                meter_hiding: &SpinnerHiding::default(),
                song_progress: SongProgress::default(),
                key_overlay: Some(KeyAnimation::Stable),
                animations,
            },
        )
    }

    fn element(component: ComponentKind, anchor: Anchor, position: [f32; 2]) -> LayoutElement {
        LayoutElement::new(component, anchor, position)
    }

    fn white() -> AtlasRegion {
        AtlasRegion {
            texture: slam_render::textures::TextureId(1),
            origin: slam_render::Origin { x: 0, y: 0 },
            size: slam_render::Extent {
                width: 4,
                height: 4,
            },
            uv: slam_render::atlas::UvRect::FULL,
        }
    }

    fn meter_textures() -> MeterTextures {
        MeterTextures {
            white: white(),
            disc: white(),
            gradient: white(),
            triangle: white(),
        }
    }

    /// The bounds of the sprites `from..to` of `batch`.
    fn bounds(batch: &SpriteBatch, from: usize, to: usize) -> ([f32; 2], [f32; 2]) {
        let vertices = &batch.vertices()[from * 4..to * 4];
        let min = vertices.iter().fold([f32::MAX; 2], |m, v| {
            [m[0].min(v.position[0]), m[1].min(v.position[1])]
        });
        let max = vertices.iter().fold([f32::MIN; 2], |m, v| {
            [m[0].max(v.position[0]), m[1].max(v.position[1])]
        });
        (min, max)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-2
    }

    #[test]
    fn counters_lay_out_their_values() {
        let mut hud = hud_with(&[], false);
        hud.update(
            1280.0,
            720.0,
            &HudValues {
                score: 12_345,
                accuracy: 0.98765,
                combo: 42,
                hits: 0,
            },
            -1.0,
        );
        assert_eq!(hud.texts(), ["012345", "98.76%", "42x"]);
        assert_eq!(hud.offset_text(), "-1 ms (hit objects appear later)");
        assert_eq!(hud.score.layout.glyphs().len(), 6);
        assert_eq!(hud.combo.layout.glyphs().len(), 3);
        // Text scales with the window: 1.5 screen units per pixel at 720 tall, a lazer
        // pixel is 1 / 1.6 of a unit.
        let font_size = hud.score.layout.font_size();
        assert!((font_size - SCORE_SIZE * 1.5 / 1.6).abs() < 1e-4);
        hud.update(1280.0, 1536.0, &HudValues::default(), 0.0);
        assert_eq!(hud.offset_text(), "0 ms");
        assert_eq!(hud.texts(), ["000000", "100.00%", "0x"]);
        // 1280 wide fits 2 units per pixel.
        assert!((hud.score.layout.font_size() - SCORE_SIZE * 2.0 / 1.6).abs() < 1e-4);
    }

    #[test]
    fn health_lands_where_the_layout_puts_it_at_any_window_size() {
        let layout = [element(
            ComponentKind::Health,
            Anchor::BottomRight,
            [-10.0, -20.0],
        )];
        let mut hud = hud(&layout);
        let snapshot = PlaySnapshot::default();
        for (width, height) in [(1920.0, 1080.0), (800.0, 600.0), (1080.0, 1920.0)] {
            hud.update(width, height, &HudValues::default(), 0.0);
            let mut batch = SpriteBatch::with_capacity(MAX_HUD_SHAPES);
            hud.push_shapes(
                &mut batch,
                &snapshot,
                0,
                Some(white()),
                None,
                &HudTextures::default(),
            );
            // The background and the fill.
            assert_eq!(batch.len(), 2);
            let unit = unit_scale(width, height);
            let size = [HEALTH_BAR[0] * unit / 1.6, HEALTH_BAR[1] * unit / 1.6];
            let (min, max) = bounds(&batch, 0, 1);
            assert!(
                close(max[0], width - 10.0 * unit),
                "{width}x{height}: {max:?}"
            );
            assert!(
                close(max[1], height - 20.0 * unit),
                "{width}x{height}: {max:?}"
            );
            assert!(close(max[0] - min[0], size[0]) && close(max[1] - min[1], size[1]));
        }
    }

    #[test]
    fn scale_rotation_and_alpha_apply_to_shapes() {
        let mut health = element(ComponentKind::Health, Anchor::Centre, [0.0, 0.0]);
        health.origin = Anchor::TopLeft;
        health.scale = [2.0, 2.0];
        health.rotation = 90.0;
        health.alpha = 0.5;
        let mut hud = hud(&[health]);
        hud.update(640.0, 480.0, &HudValues::default(), 0.0);
        let mut batch = SpriteBatch::with_capacity(MAX_HUD_SHAPES);
        hud.push_shapes(
            &mut batch,
            &PlaySnapshot::default(),
            0,
            Some(white()),
            None,
            &HudTextures::default(),
        );
        let (min, max) = bounds(&batch, 0, 1);
        // Turned clockwise about its top-left corner at the centre: the bar runs down, to
        // the left of x = 320, twice as large.
        let [length, thickness] = [HEALTH_BAR[0] / 1.6 * 2.0, HEALTH_BAR[1] / 1.6 * 2.0];
        assert!(
            close(max[0], 320.0) && close(min[0], 320.0 - thickness),
            "{min:?}"
        );
        assert!(
            close(min[1], 240.0) && close(max[1], 240.0 + length),
            "{max:?}"
        );
        assert!(
            batch.vertices()[..4]
                .iter()
                .all(|v| close(v.colour[3], SHADE.a * 0.5))
        );
    }

    #[test]
    fn counters_take_their_box_from_their_text() {
        let layout = [element(ComponentKind::Score, Anchor::TopRight, [-5.0, 2.0])];
        let mut hud = hud(&layout);
        hud.update(1920.0, 1080.0, &HudValues::default(), 0.0);
        let snapshot = PlaySnapshot::default();
        let (part, map, alpha) = hud
            .placed(&snapshot, &HudTextures::default())
            .next()
            .unwrap();
        assert_eq!((part, alpha), (Part::Score, 1.0));
        let [w, _] = hud.part_size(Part::Score, &HudTextures::default());
        assert!(w > 0.0);
        let corner = map.apply([w, 0.0]);
        assert!(close(corner[0], 1920.0 - 5.0 * 2.25) && close(corner[1], 2.0 * 2.25));
    }

    #[test]
    fn hidden_elements_and_unknown_kinds_draw_nothing() {
        let mut hidden = element(ComponentKind::Health, Anchor::TopLeft, [0.0, 0.0]);
        hidden.visible = false;
        let mut clear = element(ComponentKind::SkipButton, Anchor::TopLeft, [0.0, 0.0]);
        clear.alpha = 0.0;
        let layout = [
            hidden,
            clear,
            element(
                ComponentKind::Unknown("future".into()),
                Anchor::TopLeft,
                [0.0, 0.0],
            ),
            element(ComponentKind::PpCounter, Anchor::TopLeft, [0.0, 0.0]),
        ];
        let mut hud = hud(&layout);
        assert_eq!(hud.elements.len(), 2);
        hud.update(1280.0, 720.0, &HudValues::default(), 0.0);
        let snapshot = PlaySnapshot {
            skip_available: true,
            ..PlaySnapshot::default()
        };
        let mut batch = SpriteBatch::with_capacity(MAX_HUD_SHAPES);
        hud.push_shapes(
            &mut batch,
            &snapshot,
            0,
            Some(white()),
            Some(&meter_textures()),
            &HudTextures::default(),
        );
        assert_eq!(batch.len(), 0);
    }

    #[test]
    fn the_skip_button_shows_only_while_skipping_is_possible() {
        let layout = [element(
            ComponentKind::SkipButton,
            Anchor::BottomRight,
            [0.0, 0.0],
        )];
        let mut hud = hud(&layout);
        hud.update(1280.0, 720.0, &HudValues::default(), 0.0);
        let mut snapshot = PlaySnapshot::default();
        let mut batch = SpriteBatch::with_capacity(MAX_HUD_SHAPES);
        hud.push_shapes(
            &mut batch,
            &snapshot,
            0,
            Some(white()),
            None,
            &HudTextures::default(),
        );
        assert_eq!(batch.len(), 0);
        snapshot.skip_available = true;
        hud.push_shapes(
            &mut batch,
            &snapshot,
            0,
            Some(white()),
            None,
            &HudTextures::default(),
        );
        let (_, max) = bounds(&batch, 0, 1);
        assert!(close(max[0], 1280.0) && close(max[1], 720.0), "{max:?}");
    }

    #[test]
    fn every_hit_error_meter_of_the_layout_draws_in_its_style() {
        let mut bar = element(ComponentKind::HitErrorMeter, Anchor::CentreLeft, [0.0, 0.0]);
        bar.properties
            .insert(METER_STYLE_PROPERTY.into(), "bar".into());
        let mut mirrored = bar.clone();
        mirrored.anchor = Anchor::CentreRight;
        mirrored.scale = [-1.0, 1.0];
        let legacy = element(
            ComponentKind::HitErrorMeter,
            Anchor::BottomCentre,
            [0.0, 0.0],
        );
        let mut hud = hud(&[bar, mirrored, legacy]);
        let styles: Vec<_> = hud.meters().iter().map(HitErrorMeter::style).collect();
        assert_eq!(
            styles,
            [
                HitErrorStyle::Bar,
                HitErrorStyle::Bar,
                HitErrorStyle::Legacy
            ]
        );
        hud.update(1280.0, 720.0, &HudValues::default(), 0.0);
        let mut batch = SpriteBatch::with_capacity(MAX_HUD_SHAPES);
        let snapshot = PlaySnapshot::default();
        let textures = meter_textures();
        hud.push_shapes(
            &mut batch,
            &snapshot,
            0,
            None,
            Some(&textures),
            &HudTextures::default(),
        );
        let mut one = SpriteBatch::with_capacity(MAX_HIT_ERROR_SPRITES);
        hud.meters()[0].push(&mut one, hud.frame.scale, 0.0, &textures);
        let per_bar = one.len();
        assert!(batch.len() > 2 * per_bar);
        let (left_min, left_max) = bounds(&batch, 0, per_bar);
        let (right_min, right_max) = bounds(&batch, per_bar, 2 * per_bar);
        let (_, legacy_max) = bounds(&batch, 2 * per_bar, batch.len());
        // The left meter at the left edge, the mirrored one at the right, both centred.
        assert!(left_min[0] >= -0.01 && left_max[0] < 640.0, "{left_min:?}");
        assert!(
            right_max[0] <= 1280.01 && right_min[0] > 640.0,
            "{right_max:?}"
        );
        assert!(close(right_max[0], 1280.0 - left_min[0]));
        assert!(close((left_min[1] + left_max[1]) / 2.0, 360.0));
        assert!(close(legacy_max[1], 720.0));
    }

    #[test]
    fn text_elements_are_capped_and_bound_the_glyphs() {
        let layout: Vec<_> = (0..40)
            .map(|_| element(ComponentKind::Score, Anchor::TopLeft, [0.0, 0.0]))
            .collect();
        let hud = hud(&layout);
        assert_eq!(hud.elements.len(), MAX_HUD_TEXT_ELEMENTS);
        assert!(hud.max_glyphs() >= MAX_HUD_TEXT_ELEMENTS * NUMBER_TEXT_LEN);
        assert_eq!(
            super::tests::hud(&[]).max_glyphs(),
            hud.warm_offset
                .iter()
                .map(|l| l.glyphs().len())
                .max()
                .unwrap()
        );
    }

    fn skin_sprite(id: u32, w: f32, h: f32) -> Option<crate::circles::SkinSprite> {
        Some(crate::circles::SkinSprite {
            texture: slam_render::textures::TextureId(id),
            uv: slam_render::atlas::UvRect::FULL,
            size: Vec2::new(w, h),
        })
    }

    /// A score font of 20×40 digits (the "1" 10 wide) and symbols, a health bar of a 600×40
    /// background with the new style.
    fn skin_textures() -> HudTextures {
        let mut digits: [Option<crate::circles::SkinSprite>; 10] =
            std::array::from_fn(|d| skin_sprite(100 + d as u32, 20.0, 40.0));
        digits[1] = skin_sprite(101, 10.0, 40.0);
        let font = FontSprites {
            digits,
            symbols: [
                skin_sprite(110, 8.0, 40.0),
                skin_sprite(111, 8.0, 40.0),
                skin_sprite(112, 30.0, 40.0),
                skin_sprite(113, 24.0, 40.0),
            ],
            overlap: 0.0,
            loaded: true,
        };
        let mut health = HealthTextures {
            background: skin_sprite(1, 600.0, 40.0),
            marker: crate::health_bar::MarkerTextures::New(skin_sprite(3, 20.0, 20.0)),
            fill_frames: 1,
            ..HealthTextures::default()
        };
        health.fill[0] = skin_sprite(2, 500.0, 10.0);
        HudTextures {
            score: font,
            combo: font,
            health,
            ..HudTextures::default()
        }
    }

    #[test]
    fn counters_and_health_draw_with_the_skin_when_it_has_them() {
        let layout = [
            element(ComponentKind::Score, Anchor::TopRight, [0.0, 0.0]),
            element(ComponentKind::Combo, Anchor::BottomLeft, [0.0, 0.0]),
            element(ComponentKind::Health, Anchor::TopLeft, [0.0, 0.0]),
        ];
        let mut hud = hud(&layout);
        let textures = skin_textures();
        // 1600 × 1200: 2.5 window pixels per unit, a lazer pixel is 1.5625.
        let (width, height) = (1600.0, 1200.0);
        let values = HudValues {
            score: 1_000,
            combo: 12,
            ..HudValues::default()
        };
        hud.update(width, height, &values, 0.0);
        let mut snapshot = PlaySnapshot {
            hud: values,
            health: 1.0,
            ..PlaySnapshot::default()
        };
        hud.animate(&snapshot, 0);
        snapshot.hud.hits = 1;
        hud.animate(&snapshot, 100_000_000);
        let mut batch = SpriteBatch::with_capacity(MAX_HUD_SHAPES);
        hud.push_shapes(&mut batch, &snapshot, 0, Some(white()), None, &textures);
        // The counters in the skin font are not text.
        assert!(textures.font(Part::Score).is_some() && textures.font(Part::Combo).is_some());

        let pixel = 2.5 / 1.6;
        // "001000" at a fixed width of 20, scaled by 0.96, its right edge on the window's.
        let (min, max) = bounds(&batch, 0, 6);
        assert!(close(max[0], width), "{max:?}");
        assert!(
            close(max[0] - min[0], 120.0 * 0.96 * pixel),
            "{min:?} {max:?}"
        );
        assert!(close(min[1], 0.0) && close(max[1], 40.0 * 0.96 * pixel));
        // The combo shows "12x" (it was loaded at 12): 20 + 10 + 24 wide at 1.28, bottom
        // left, no pop out.
        let (min, max) = bounds(&batch, 6, 9);
        assert!(
            close(min[0], 0.0) && close(max[1], height),
            "{min:?} {max:?}"
        );
        assert!(close(max[0] - min[0], 54.0 * 1.28 * pixel));
        // The health bar: the background at the top left, the marker and its flash; the fill
        // is still empty, filling up from the start.
        let (min, max) = bounds(&batch, 9, 10);
        assert!(close(min[0], 0.0) && close(min[1], 0.0));
        assert!(close(max[0], 600.0 * pixel) && close(max[1], 40.0 * pixel));
        assert_eq!(batch.len(), 12);
    }

    #[test]
    fn the_combo_pops_out_on_increase() {
        let layout = [element(
            ComponentKind::Combo,
            Anchor::BottomLeft,
            [0.0, 0.0],
        )];
        let mut hud = hud(&layout);
        let textures = skin_textures();
        hud.update(1600.0, 1200.0, &HudValues::default(), 0.0);
        let mut snapshot = PlaySnapshot::default();
        hud.animate(&snapshot, 0);
        snapshot.hud.combo = 1;
        hud.animate(&snapshot, 1_000_000);
        let mut batch = SpriteBatch::with_capacity(MAX_HUD_SHAPES);
        hud.push_shapes(&mut batch, &snapshot, 0, None, None, &textures);
        // The pop out "1x" (additive, drawn first), then the counter's "0x".
        assert_eq!(batch.len(), 4);
        assert_eq!(batch.batches()[0].blend, Blend::Additive);
        // The pop out starts 1.56 times as big and reaches 3 units of the counter to the
        // left of it.
        let unit = 1.28 * 2.5 / 1.6;
        let (pop_min, pop_max) = bounds(&batch, 0, 2);
        let (min, max) = bounds(&batch, 2, 4);
        assert!(close(pop_max[0] - pop_min[0], (10.0 + 24.0) * 1.56 * unit));
        assert!(
            close(pop_min[0], min[0] - 3.0 * 1.56 * unit),
            "{pop_min:?} {min:?}"
        );
        assert!(close(max[1] - min[1], 40.0 * unit));
    }

    #[test]
    fn attached_elements_follow_their_target_whatever_its_size() {
        use slam_ui::hud_layout::Attach;
        let mut accuracy = element(ComponentKind::Accuracy, Anchor::TopRight, [-4.0, 2.0]);
        accuracy.attach = Some(Attach {
            to: ComponentKind::Score,
            point: Anchor::BottomRight,
        });
        let mut progress = element(ComponentKind::SongProgress, Anchor::TopRight, [-10.0, 0.0]);
        progress.origin = Anchor::CentreRight;
        progress.attach = Some(Attach {
            to: ComponentKind::Accuracy,
            point: Anchor::CentreLeft,
        });
        let layout = [
            element(ComponentKind::Score, Anchor::TopRight, [0.0, 0.0]),
            accuracy,
            progress,
        ];
        let mut hud = hud(&layout);
        // 1280 × 960: 2 window pixels per unit.
        hud.update(1280.0, 960.0, &HudValues::default(), 0.0);
        let snapshot = PlaySnapshot::default();
        for digit_height in [40.0, 80.0] {
            let mut textures = skin_textures();
            for d in textures.score.digits.iter_mut().flatten() {
                d.size.y = digit_height;
            }
            let boxes: Vec<_> = hud.placed(&snapshot, &textures).collect();
            let score_size = hud.part_size(Part::Score, &textures);
            let accuracy_size = hud.part_size(Part::Accuracy, &textures);
            let score_bottom = boxes[0].1.apply([score_size[0], score_size[1]]);
            assert!(close(score_bottom[1], digit_height * 0.96 * 2.0 / 1.6));
            // The accuracy's top right: the score's bottom right plus its offset.
            let top_right = boxes[1].1.apply([accuracy_size[0], 0.0]);
            assert!(close(top_right[0], 1280.0 - 8.0), "{top_right:?}");
            assert!(close(top_right[1], score_bottom[1] + 4.0), "{top_right:?}");
            // The progress's centre right: 20 pixels left of the accuracy's centre left.
            let side = crate::song_progress::SIZE * 2.0 / 1.6;
            let centre_right = boxes[2].1.apply([side, side / 2.0]);
            let centre_left = boxes[1].1.apply([0.0, accuracy_size[1] / 2.0]);
            assert!(close(centre_right[0], centre_left[0] - 20.0));
            assert!(close(centre_right[1], centre_left[1]));
        }
    }

    #[test]
    fn a_font_without_images_hides_its_counter() {
        let layout = [element(
            ComponentKind::Combo,
            Anchor::BottomLeft,
            [0.0, 0.0],
        )];
        let mut hud = hud(&layout);
        hud.update(1280.0, 720.0, &HudValues::default(), 0.0);
        let mut snapshot = PlaySnapshot::default();
        snapshot.hud.combo = 5;
        hud.animate(&snapshot, 0);
        let mut textures = skin_textures();
        textures.combo = FontSprites {
            loaded: true,
            ..FontSprites::default()
        };
        let mut batch = SpriteBatch::with_capacity(MAX_HUD_SHAPES);
        hud.push_shapes(&mut batch, &snapshot, 0, Some(white()), None, &textures);
        assert_eq!(batch.len(), 0);
        // Not text either.
        assert!(textures.font(Part::Combo).is_some());
        // Until the font is loaded, the counter is text.
        textures.combo.loaded = false;
        assert!(textures.font(Part::Combo).is_none());
    }

    #[test]
    fn an_element_attached_to_a_missing_target_takes_the_screen_and_a_hidden_one_counts() {
        use slam_ui::hud_layout::Attach;
        let mut accuracy = element(ComponentKind::Accuracy, Anchor::TopRight, [0.0, 0.0]);
        accuracy.attach = Some(Attach {
            to: ComponentKind::Score,
            point: Anchor::BottomRight,
        });
        // No score before it: from the screen's top right.
        let mut hud = hud(std::slice::from_ref(&accuracy));
        hud.update(1280.0, 960.0, &HudValues::default(), 0.0);
        let snapshot = PlaySnapshot::default();
        let textures = skin_textures();
        let (_, map, _) = hud.placed(&snapshot, &textures).next().unwrap();
        let size = hud.part_size(Part::Accuracy, &textures);
        assert!(close(map.apply([size[0], 0.0])[1], 0.0));
        // A score after it does not count either.
        let later = [
            accuracy.clone(),
            element(ComponentKind::Score, Anchor::TopRight, [0.0, 0.0]),
        ];
        let mut hud = self::hud(&later);
        hud.update(1280.0, 960.0, &HudValues::default(), 0.0);
        let (_, map, _) = hud.placed(&snapshot, &textures).next().unwrap();
        assert!(close(map.apply([size[0], 0.0])[1], 0.0));
        // A hidden, scaled score still places it: its bottom right, at twice the size.
        let mut score = element(ComponentKind::Score, Anchor::TopRight, [0.0, 0.0]);
        score.visible = false;
        score.scale = [2.0, 2.0];
        let mut hud = self::hud(&[score, accuracy]);
        hud.update(1280.0, 960.0, &HudValues::default(), 0.0);
        let placed: Vec<_> = hud.placed(&snapshot, &textures).collect();
        assert_eq!(placed.len(), 1);
        let score_height = hud.part_size(Part::Score, &textures)[1];
        let top_right = placed[0].1.apply([size[0], 0.0]);
        assert!(close(top_right[0], 1280.0) && close(top_right[1], 2.0 * score_height));
    }

    #[test]
    fn the_key_overlay_shows_only_when_turned_on() {
        let mut overlay = element(ComponentKind::KeyCounter, Anchor::CentreRight, [0.0, -40.0]);
        overlay.origin = Anchor::TopRight;
        let layout = [overlay];
        let off = HudView::new(
            Arc::new(FontRegistry::with_builtin()),
            &labels(),
            HudSetup {
                layout: &layout,
                windows: &HitWindows::new(5.0),
                meter_hiding: &SpinnerHiding::default(),
                song_progress: SongProgress::default(),
                key_overlay: None,
                animations: true,
            },
        );
        assert!(off.elements.is_empty());

        let mut hud = hud(&layout);
        hud.update(1280.0, 960.0, &HudValues::default(), 0.0);
        let mut snapshot = PlaySnapshot::default();
        hud.animate(&snapshot, 0);
        snapshot.keys.presses[0] = 1;
        snapshot.keys.counts[0] = 1;
        snapshot.keys.held[0] = true;
        hud.animate(&snapshot, 10_000_000);
        let mut textures = skin_textures();
        textures.keys = KeyOverlayTextures {
            background: skin_sprite(20, 193.0, 55.0),
            key: skin_sprite(21, 43.0, 46.0),
        };
        let mut batch = SpriteBatch::with_capacity(MAX_HUD_SHAPES);
        hud.push_shapes(&mut batch, &snapshot, 0, Some(white()), None, &textures);
        // The background and four keys, the right edge on the window's, its top 40 units
        // above the middle.
        assert_eq!(batch.len(), 5);
        let (min, max) = bounds(&batch, 0, 5);
        assert!(close(max[0], 1280.0 + 0.5 * 1.25), "{max:?}");
        assert!(close(min[1], 480.0 - 80.0), "{min:?}");
        // K1 is pressed: yellow; K2 is not.
        let colour = |i: usize| batch.vertices()[i * 4].colour;
        assert!(colour(1)[2] < 0.5 && colour(2)[2] > 0.99);
        // The captions turn into counts.
        assert_eq!(hud.key_texts[0].text.as_str(), "1");
        assert_eq!(hud.key_texts[1].text.as_str(), "K2");
    }

    #[test]
    fn a_skinned_skip_button_is_its_image() {
        let layout = [element(
            ComponentKind::SkipButton,
            Anchor::BottomRight,
            [0.0, 0.0],
        )];
        let mut hud = hud(&layout);
        hud.update(1280.0, 960.0, &HudValues::default(), 0.0);
        let snapshot = PlaySnapshot {
            skip_available: true,
            ..PlaySnapshot::default()
        };
        let mut textures = skin_textures();
        textures.skip.frames[0] = skin_sprite(30, 193.0, 147.0);
        textures.skip.count = 1;
        let mut batch = SpriteBatch::with_capacity(MAX_HUD_SHAPES);
        hud.push_shapes(&mut batch, &snapshot, 0, Some(white()), None, &textures);
        assert_eq!(batch.len(), 1);
        let (min, max) = bounds(&batch, 0, 1);
        assert!(close(max[0], 1280.0) && close(max[1], 960.0));
        assert!(close(max[0] - min[0], 193.0 * 1.25));
        assert!(textures.skip.frame(0.0).is_some());
    }

    #[test]
    fn score_and_accuracy_roll_to_new_values_unless_animations_are_off() {
        let mut hud = hud(&[]);
        let values = |score, accuracy| HudValues {
            score,
            accuracy,
            ..HudValues::default()
        };
        let mut snapshot = PlaySnapshot {
            hud: values(1000, 1.0),
            ..PlaySnapshot::default()
        };
        // A frame: the values, then the animations.
        let frame = |hud: &mut HudView, snapshot: &PlaySnapshot, ms: u64| {
            hud.update(1280.0, 720.0, &snapshot.hud, 0.0);
            hud.animate(snapshot, ms * 1_000_000);
        };
        frame(&mut hud, &snapshot, 0);
        assert_eq!(hud.texts()[..2], ["001000", "100.00%"]);
        // The frame a new value comes in still shows the old one.
        snapshot.hud = values(3000, 0.5);
        frame(&mut hud, &snapshot, 100);
        assert_eq!(hud.texts()[..2], ["001000", "100.00%"]);
        // Half of the score's 1000 ms, Easing.Out: three quarters of the way; the accuracy's
        // 375 ms are over.
        frame(&mut hud, &snapshot, 600);
        assert_eq!(hud.texts()[..2], ["002500", "50.00%"]);
        // A new value mid-roll goes on from what showed.
        snapshot.hud = values(5000, 0.5);
        frame(&mut hud, &snapshot, 700);
        assert_eq!(hud.texts()[0], "002500");
        frame(&mut hud, &snapshot, 1700);
        assert_eq!(hud.texts()[0], "005000");
        // A new attempt starts at its values.
        snapshot.restarts = 1;
        snapshot.hud = values(0, 1.0);
        frame(&mut hud, &snapshot, 1710);
        assert_eq!(hud.texts()[..2], ["000000", "100.00%"]);

        let mut instant = hud_with(&[], false);
        instant.update(1280.0, 720.0, &values(1000, 1.0), 0.0);
        instant.update(1280.0, 720.0, &values(3000, 0.5), 0.0);
        assert_eq!(instant.texts()[..2], ["003000", "50.00%"]);
        // The combo shows at once too.
        snapshot.hud.combo = 7;
        instant.update(1280.0, 720.0, &snapshot.hud, 0.0);
        instant.animate(&snapshot, 0);
        let pose = instant.combo_pose().unwrap();
        assert_eq!((pose.shown, pose.shown_alpha, pose.pop_out), (7, 1.0, None));
    }
}
