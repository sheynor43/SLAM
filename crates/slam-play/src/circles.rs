//! Hit circles as lazer draws them with a legacy skin: the circle, its overlay and combo number,
//! and the approach circle.
//!
//! The update side copies the circles alive in [`OsuRules`] into a [`LiveCircles`] snapshot. The
//! draw side keeps the static [`CircleVisuals`] built at load, evaluates each live circle's
//! transforms at the frame's game time ([`circle_pose`]) and writes sprites
//! ([`push_circles`]). Everything per frame is a pure function of the game time and the
//! judgements, without allocations.
//!
//! Transforms follow osu-framework's rules as seen at a high frame rate: adding a transform
//! removes the transforms of the same property that start after it, a transform starts from
//! the value its property has at its start time, and a later-starting transform replaces any
//! earlier one still running.
//!
//! Not done: the kiai flash (`LegacyKiaiFlashingDrawable`), the shake on an early press, the
//! combo colour brightness setting (`ComboColourNormalisationAmount`, 0 by default), turning hit
//! animations off (on by default), and textures from the beatmap's skin.

use slam_osu::hit_windows::HitWindows;
use slam_osu::judgement::HitResult;
use slam_osu::mods::HIDDEN_FADE_OUT_MULTIPLIER;
use slam_osu::objects::{ComboInfo, OsuHitObjectKind};
use slam_osu::rules::{CircleJudgement, LiveCircle, OsuRules};
use slam_osu::{Beatmap, Mod, ModSet, Vec2};
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::textures::TextureId;
use slam_render::{Blend, Color};
use slam_skin::{Rgba, Skin, SkinChain, SkinSource, SkinVersion};

use crate::legacy_text::{Digits, LegacyFont};
use crate::playfield::PlayfieldLayout;

/// The most circles a [`LiveCircles`] snapshot holds; later ones are left out.
pub const MAX_LIVE_CIRCLES: usize = 256;

/// Lazer's `OsuHitObject.OBJECT_RADIUS` doubled: the size of a circle at scale 1, in
/// osu!pixels.
pub(crate) const OBJECT_SIZE: f32 = 128.0;

/// `LegacyMainCirclePiece`'s `maxSize`: circle textures are cropped to this display size.
pub(crate) const MAX_CIRCLE_SIZE: f32 = OBJECT_SIZE * 2.0;

/// Stable's blanket scale of hit circle numbers (`hitcircle_text_scale`).
const NUMBER_SCALE: f32 = 0.8;

/// Digit textures are cropped to this display size (`MaxSizePerGlyph`).
const MAX_DIGIT_SIZE: f32 = MAX_CIRCLE_SIZE / NUMBER_SCALE;

/// `LegacyMainCirclePiece`'s `legacy_fade_duration`.
pub(crate) const LEGACY_FADE_DURATION: f64 = 240.0;

/// The colour pieces are dimmed to until shortly before they can be hit.
pub(crate) const DIM: f32 = 195.0 / 255.0;

/// Lazer's `OsuHitWindows.MISS_WINDOW`.
pub(crate) const MISS_WINDOW: f64 = 400.0;

// ---- Snapshot ----

/// The circles alive at an update, copied for the draw side without allocations.
/// Not `Copy`: it is large, copy it into a buffer slot with `clone_from`.
#[derive(Debug, Clone)]
pub struct LiveCircles {
    circles: [LiveCircle; MAX_LIVE_CIRCLES],
    len: usize,
    /// Circles left out because the snapshot was full.
    dropped: usize,
}

impl Default for LiveCircles {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveCircles {
    /// An empty snapshot.
    pub const fn new() -> Self {
        LiveCircles {
            circles: [LiveCircle {
                index: 0,
                judgement: None,
            }; MAX_LIVE_CIRCLES],
            len: 0,
            dropped: 0,
        }
    }

    /// Replaces the contents with the circles alive at the last update of `rules`.
    pub fn capture(&mut self, rules: &OsuRules) {
        self.len = 0;
        self.dropped = 0;
        for circle in rules.live_circles() {
            if self.len < MAX_LIVE_CIRCLES {
                self.circles[self.len] = circle;
                self.len += 1;
            } else {
                self.dropped += 1;
            }
        }
    }

    /// Empties the snapshot.
    pub fn clear(&mut self) {
        self.len = 0;
        self.dropped = 0;
    }

    /// The circles, in start time order.
    pub fn as_slice(&self) -> &[LiveCircle] {
        &self.circles[..self.len]
    }

    /// How many alive circles did not fit at the last capture.
    pub fn dropped(&self) -> usize {
        self.dropped
    }
}

// ---- Static data ----

/// What the mods change about how circles look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircleMods {
    /// Hidden, with whether only the approach circles are hidden
    /// (`OnlyFadeApproachCircles`).
    pub hidden: Option<bool>,
    /// Traceable: circle pieces are hidden, approach circles stay.
    pub traceable: bool,
    /// Classic's `FadeHitCircleEarly`, off when Hidden fades the circles.
    pub fade_early: bool,
}

impl CircleMods {
    /// The visual effects of `mods`.
    pub fn new(mods: &ModSet) -> Self {
        let hidden = mods.mods().iter().find_map(|m| match m {
            Mod::Hidden {
                only_fade_approach_circles,
            } => Some(*only_fade_approach_circles),
            _ => None,
        });
        let traceable = mods.mods().iter().any(|m| matches!(m, Mod::Traceable));
        let classic_early = mods
            .mods()
            .iter()
            .any(|m| matches!(m, Mod::Classic(c) if c.fade_hit_circle_early));
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModClassic.cs (usingHiddenFading)
        let hidden_fading = hidden == Some(false);
        CircleMods {
            hidden,
            traceable,
            fade_early: classic_early && !hidden_fading,
        }
    }

    /// Whether a visibility mod (Hidden or Traceable) is on, which the first object can be
    /// spared from (`IncreaseFirstObjectVisibility`).
    pub fn adjusts_visibility(&self) -> bool {
        self.hidden.is_some() || self.traceable
    }
}

/// The player's settings that change how circles look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircleSettings {
    /// Lazer's `BeatmapColours`: combo colours come from the beatmap when it has them.
    pub beatmap_colours: bool,
    /// Lazer's `IncreaseFirstObjectVisibility`: under Hidden, the first object keeps its
    /// approach circle and only its circle fades; Traceable leaves it alone.
    pub increase_first_object_visibility: bool,
}

impl Default for CircleSettings {
    /// Lazer's defaults: both on.
    fn default() -> Self {
        CircleSettings {
            beatmap_colours: true,
            increase_first_object_visibility: true,
        }
    }
}

/// One hit circle's static drawing data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircleVisual {
    pub start_time: f64,
    /// Stacked position, in osu!pixels.
    pub position: Vec2,
    /// Lazer's `OsuHitObject.Scale`.
    pub scale: f32,
    pub time_preempt: f64,
    pub time_fade_in: f64,
    /// The accent colour: the combo colour, never fully transparent.
    pub colour: Color,
    /// The combo number shown (`IndexInCurrentCombo + 1`).
    pub number: u32,
    /// Whether Hidden or Traceable spares this circle as the first object (increased
    /// visibility).
    pub increased_visibility: bool,
}

/// How every circle of a play is drawn, apart from the textures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircleStyle {
    pub mods: CircleMods,
    /// The Ok and Meh hit windows, for Classic's early fade.
    pub ok_window: f64,
    pub meh_window: f64,
    /// Skins of version 2.0 and newer only briefly fade the number out on a hit; older ones
    /// scale and fade it with the circle.
    pub number_short_fade: bool,
}

impl CircleStyle {
    /// The style for a play with `mods` and `windows`, drawn with `skins`.
    pub fn new(mods: CircleMods, windows: &HitWindows, skins: &SkinChain) -> Self {
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyMainCirclePiece.cs (updateStateTransforms)
        let version = skins.config(Skin::config_version).map(|f| f.value);
        CircleStyle {
            mods,
            ok_window: windows.window_for(HitResult::Ok),
            meh_window: windows.window_for(HitResult::Meh),
            number_short_fade: version.is_some_and(|v| v > SkinVersion::V1),
        }
    }
}

/// The static drawing data of every hit circle of a beatmap, by object index.
#[derive(Debug, Clone)]
pub struct CircleVisuals {
    circles: Vec<Option<CircleVisual>>,
}

impl CircleVisuals {
    /// The circles of `beatmap` (with its mods applied), coloured from `beatmap_skin` (the
    /// beatmap's own [`Skin::load_beatmap`], if any) and `skins`.
    pub fn new(
        beatmap: &Beatmap,
        mods: CircleMods,
        settings: CircleSettings,
        beatmap_skin: Option<&Skin>,
        skins: &SkinChain,
    ) -> Self {
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModWithVisibilityAdjustment.cs (ApplyToBeatmap)
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModHidden.cs (IsFirstAdjustableObject)
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModTraceable.cs (IsFirstAdjustableObject)
        let first = beatmap.hit_objects.iter().position(|o| !o.is_spinner());
        let increase_first = mods.adjusts_visibility() && settings.increase_first_object_visibility;
        let beatmap_colours =
            beatmap_skin.filter(|s| settings.beatmap_colours && !s.ini().combo_colours.is_empty());
        let circles = beatmap
            .hit_objects
            .iter()
            .enumerate()
            .map(|(i, o)| match o.kind {
                OsuHitObjectKind::Circle => Some(CircleVisual {
                    start_time: o.start_time,
                    position: o.stacked_position(),
                    scale: o.defaults.scale,
                    time_preempt: o.defaults.time_preempt,
                    time_fade_in: o.defaults.time_fade_in,
                    colour: combo_colour(&o.combo, beatmap_colours, skins),
                    number: u32::try_from(o.combo.index_in_current_combo + 1).unwrap_or(0),
                    increased_visibility: increase_first && first == Some(i),
                }),
                _ => None,
            })
            .collect();
        CircleVisuals { circles }
    }

    /// The circle with object index `index`; `None` for other objects.
    pub fn get(&self, index: u32) -> Option<&CircleVisual> {
        self.circles.get(index as usize)?.as_ref()
    }
}

/// The colour of an object's combo: the beatmap's colours by the index with offsets when it has
/// any, else the first skin's (whose fallback is the default four) by the combo index.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Types/IHasComboInformation.cs (GetComboColour)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkin.cs (GetComboColour)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyBeatmapSkin.cs (GetComboColour)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyColourCompatibility.cs (DisallowZeroAlpha)
pub(crate) fn combo_colour(combo: &ComboInfo, beatmap: Option<&Skin>, skins: &SkinChain) -> Color {
    let [r, g, b, a] = match beatmap {
        // A beatmap skin has no default colours to fall back on; the caller checked it has some.
        Some(skin) => pick(&skin.ini().combo_colours, combo.combo_index_with_offsets),
        // Combo colours of the beatmap layer are gated by `BeatmapColours`, not by the chain.
        None => pick(
            skins.skin(SkinSource::User).combo_colours(),
            combo.combo_index,
        ),
    };
    let a = if a == 0 { 255 } else { a };
    Color {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: f32::from(a) / 255.0,
    }
}

/// `ComboColours[index % Count]`.
fn pick(colours: &[Rgba], index: i32) -> [u8; 4] {
    // Combo indices start at 1 and never decrease.
    let c = colours[index.rem_euclid(colours.len() as i32) as usize];
    [c.r, c.g, c.b, c.a]
}

// ---- Transforms ----

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Easing {
    None,
    /// osu-framework's `Easing.In` (quadratic).
    In,
    /// osu-framework's `Easing.Out` (quadratic).
    Out,
    OutQuint,
    InQuint,
    OutElasticHalf,
}

impl Easing {
    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/DefaultEasingFunction.cs (ApplyEasing)
    pub(crate) fn apply(self, t: f64) -> f64 {
        const ELASTIC_CONST: f64 = 2.0 * std::f64::consts::PI / 0.3;
        const ELASTIC_CONST2: f64 = 0.3 / 4.0;
        match self {
            Easing::None => t,
            Easing::In => t * t,
            Easing::Out => t * (2.0 - t),
            Easing::OutQuint => {
                let t = t - 1.0;
                t * t * t * t * t + 1.0
            }
            Easing::InQuint => t * t * t * t * t,
            Easing::OutElasticHalf => {
                let offset = 2f64.powi(-10) * ((0.5 - ELASTIC_CONST2) * ELASTIC_CONST).sin();
                2f64.powf(-10.0 * t) * ((0.5 * t - ELASTIC_CONST2) * ELASTIC_CONST).sin() + 1.0
                    - offset * t
            }
        }
    }
}

/// osu-framework's `Interpolation.ValueAt` of a `float` (interpolated in `double`), without
/// clamping `time` to the range.
// Ported from osu-framework 2026.921.1: osu.Framework/Utils/Interpolation.cs (ValueAt)
pub(crate) fn value_at(time: f64, from: f32, to: f32, start: f64, end: f64, easing: Easing) -> f32 {
    let (from, to) = (f64::from(from), f64::from(to));
    if from == to {
        return from as f32;
    }
    let current = time - start;
    let duration = end - start;
    if current == 0.0 {
        return from as f32;
    }
    if duration == 0.0 {
        return to as f32;
    }
    (from + easing.apply(current / duration) * (to - from)) as f32
}

/// One transform of a single property.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Tween {
    start: f64,
    duration: f64,
    end: f32,
    easing: Easing,
}

impl Tween {
    const fn new(start: f64, duration: f64, end: f32, easing: Easing) -> Self {
        Tween {
            start,
            duration,
            end,
            easing,
        }
    }

    fn ease(&self, t: f64) -> f64 {
        self.easing.apply(t)
    }

    /// The value at `time` (not before the start) when starting from `from`, interpolated as
    /// osu-framework interpolates properties of type `kind`.
    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/Transform.cs
    // Ported from osu-framework 2026.921.1: osu.Framework/Utils/Interpolation.cs (ValueAt, ApplyEasing)
    fn value_at(&self, kind: Kind, from: f32, time: f64) -> f32 {
        if time >= self.start + self.duration {
            return self.end;
        }
        match kind {
            Kind::Alpha => {
                let t = self.ease((time - self.start) / self.duration);
                let from = f64::from(from);
                (from + t * (f64::from(self.end) - from)) as f32
            }
            Kind::Scale => {
                let current = (time - self.start) as f32;
                let duration = self.duration as f32;
                if current == 0.0 {
                    return from;
                }
                let t = self.ease(f64::from(current / duration)) as f32;
                from + t * (self.end - from)
            }
            Kind::Colour => {
                let current = time - self.start;
                if from == self.end || current == 0.0 {
                    return from;
                }
                let t = (self.ease(current / self.duration) as f32).clamp(0.0, 1.0);
                let (a, b) = (to_linear(from), to_linear(self.end));
                to_srgb(a + t * (b - a))
            }
        }
    }
}

/// How osu-framework interpolates a property.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    /// A `float`, interpolated in `double`.
    Alpha,
    /// A `Vector2`, interpolated in `float`.
    Scale,
    /// One channel of a grey `Color4`, interpolated in linear space.
    Colour,
}

/// osu-framework's gamma of sRGB.
const GAMMA: f32 = 2.4;

// Ported from osu-framework 2026.921.1: osu.Framework/Extensions/Color4Extensions/Color4Extensions.cs (ToLinear)
pub(crate) fn to_linear(colour: f32) -> f32 {
    if colour == 1.0 {
        1.0
    } else if colour <= 0.04045 {
        colour / 12.92
    } else {
        ((colour + 0.055) / 1.055).powf(GAMMA)
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Extensions/Color4Extensions/Color4Extensions.cs (ToSRGB)
pub(crate) fn to_srgb(colour: f32) -> f32 {
    if colour == 1.0 {
        1.0
    } else if colour < 0.003_130_8 {
        12.92 * colour
    } else {
        1.055 * colour.powf(1.0 / GAMMA) - 0.055
    }
}

/// Most transforms a [`Track`] holds: a legacy judgement's scale takes five.
const MAX_TWEENS: usize = 6;

/// The transforms of one property, by start time. A circle's properties hold at most three at
/// once (the approach circle's alpha: its fade-in, the fade at the start time and the fade on
/// a hit); a legacy judgement's scale holds five.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Track {
    kind: Kind,
    tweens: [Tween; MAX_TWEENS],
    len: usize,
}

impl Track {
    pub(crate) const fn new(kind: Kind) -> Self {
        Track {
            kind,
            tweens: [Tween::new(0.0, 0.0, 0.0, Easing::None); MAX_TWEENS],
            len: 0,
        }
    }

    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/TargetGroupingTransformTracker.cs (AddTransform)
    pub(crate) fn add(&mut self, start: f64, duration: f64, end: f32, easing: Easing) {
        // Inserted after the transforms starting no later (osu-framework orders by start time,
        // then by creation); the ones starting after it are removed.
        let i = self.tweens[..self.len]
            .iter()
            .take_while(|t| t.start <= start)
            .count();
        assert!(i < self.tweens.len(), "too many transforms on one property");
        self.tweens[i] = Tween::new(start, duration, end, easing);
        self.len = i + 1;
    }

    /// Drops the transforms superseded by `time`: all but the latest started by then, which
    /// starts from `initial`, updated to the value the property had when it started. Lets a
    /// long series of transforms, added in time order, fit.
    pub(crate) fn settle(&mut self, initial: &mut f32, time: f64) {
        let started = self.tweens[..self.len]
            .iter()
            .take_while(|t| t.start <= time)
            .count();
        if started < 2 {
            return;
        }
        let last = started - 1;
        // As `value` steps from one transform to the next.
        for j in 1..=last {
            *initial = self.tweens[j - 1].value_at(self.kind, *initial, self.tweens[j].start);
        }
        self.tweens.copy_within(last..self.len, 0);
        self.len -= last;
    }

    pub(crate) fn fade_out(&mut self, start: f64, duration: f64) {
        self.add(start, duration, 0.0, Easing::None);
    }

    /// The value at `time` of a property starting at `initial`: the latest transform started by
    /// then, from the value the property had when it started.
    pub(crate) fn value(&self, initial: f32, time: f64) -> f32 {
        let mut value = initial;
        let mut active: Option<&Tween> = None;
        for tween in &self.tweens[..self.len] {
            if tween.start > time {
                break;
            }
            if let Some(a) = active {
                value = a.value_at(self.kind, value, tween.start);
            }
            active = Some(tween);
        }
        active.map_or(value, |a| a.value_at(self.kind, value, time))
    }
}

/// A circle's appearance at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CirclePose {
    /// The alpha of the whole drawable (`DrawableHitCircle`), approach circle included.
    pub object_alpha: f32,
    /// The alpha of the circle piece: the fade-in.
    pub piece_alpha: f32,
    /// The brightness the circle piece is tinted with: dimmed until shortly before it can be
    /// hit.
    pub brightness: f32,
    /// The alpha of the circle and overlay sprites.
    pub sprite_alpha: f32,
    /// The scale of the circle and overlay sprites.
    pub sprite_scale: f32,
    pub number_alpha: f32,
    pub number_scale: f32,
    pub approach_alpha: f32,
    pub approach_scale: f32,
}

/// The appearance of `circle` at game `time`, judged as `judgement`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (UpdateInitialTransforms, UpdateStartTimeStateTransforms, UpdateHitStateTransforms)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableOsuHitObject.cs (applyDim)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Drawables/DrawableHitObject.cs (UpdateState, HitStateUpdateTime)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyMainCirclePiece.cs (updateStateTransforms)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModHidden.cs (applyHiddenState, getFadeOutParameters)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModClassic.cs (applyEarlyFading)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModTraceable.cs (applyCirclePieceState)
pub fn circle_pose(
    circle: &CircleVisual,
    judgement: Option<CircleJudgement>,
    time: f64,
    style: &CircleStyle,
) -> CirclePose {
    let start = circle.start_time;
    let preempt = circle.time_preempt;
    let fade_in = circle.time_fade_in;
    let appear = start - preempt;
    // An unjudged circle's hit state time is its end time, which is its start time.
    let hit_time = judgement.map_or(start, |j| j.time);
    let hit = judgement.is_some_and(|j| j.result.is_hit());

    let mut object = Track::new(Kind::Alpha);
    let mut piece = Track::new(Kind::Alpha);
    let mut brightness = Track::new(Kind::Colour);
    let mut approach_alpha = Track::new(Kind::Alpha);
    let mut approach_scale = Track::new(Kind::Scale);
    let mut sprite_alpha = Track::new(Kind::Alpha);
    let mut sprite_scale = Track::new(Kind::Scale);
    let mut number_alpha = Track::new(Kind::Alpha);
    let mut number_scale = Track::new(Kind::Scale);

    // Initial transforms, from the start of the preempt.
    brightness.add(appear, 0.0, DIM, Easing::None);
    brightness.add(appear + (preempt - MISS_WINDOW), 100.0, 1.0, Easing::None);
    piece.add(appear, fade_in, 1.0, Easing::None);
    approach_alpha.add(appear, (fade_in * 2.0).min(preempt), 0.9, Easing::None);
    approach_scale.add(appear, preempt, 1.0, Easing::None);

    // Start time transforms.
    approach_alpha.fade_out(start, 50.0);

    // Hit state transforms; hit animations are on.
    object.fade_out(hit_time + 800.0, 0.0);
    match judgement {
        Some(_) if hit => {
            approach_alpha.fade_out(hit_time, 0.0);
            sprite_alpha.fade_out(hit_time, LEGACY_FADE_DURATION);
            sprite_scale.add(hit_time, LEGACY_FADE_DURATION, 1.4, Easing::Out);
            if style.number_short_fade {
                number_alpha.fade_out(hit_time, LEGACY_FADE_DURATION / 4.0);
            } else {
                number_alpha.fade_out(hit_time, LEGACY_FADE_DURATION);
                number_scale.add(hit_time, LEGACY_FADE_DURATION, 1.4, Easing::Out);
            }
        }
        Some(_) => object.fade_out(hit_time, 100.0),
        None => {}
    }

    // Custom state: Hidden.
    if let Some(only_approach) = style.mods.hidden {
        if !circle.increased_visibility {
            approach_alpha.fade_out(appear, 0.0);
        }
        if !only_approach {
            let fade_start = appear + fade_in;
            let fade_duration = preempt * HIDDEN_FADE_OUT_MULTIPLIER;
            if circle.increased_visibility {
                piece.fade_out(fade_start, fade_duration);
            } else {
                object.fade_out(fade_start, fade_duration);
            }
        }
    }

    // Custom state: Traceable hides the circle piece from the start of the preempt; the first
    // object keeps it.
    if style.mods.traceable && !circle.increased_visibility {
        piece.fade_out(appear, 0.0);
    }

    // Custom state: Classic's early fade, into the miss.
    if style.mods.fade_early && !hit {
        object.fade_out(start + style.ok_window, style.meh_window - style.ok_window);
    }

    CirclePose {
        object_alpha: object.value(1.0, time),
        // `FadeInFromZero`: transparent before the preempt.
        piece_alpha: piece.value(0.0, time),
        brightness: brightness.value(1.0, time),
        sprite_alpha: sprite_alpha.value(1.0, time),
        sprite_scale: sprite_scale.value(1.0, time),
        number_alpha: number_alpha.value(1.0, time),
        number_scale: number_scale.value(1.0, time),
        approach_alpha: approach_alpha.value(0.0, time),
        approach_scale: approach_scale.value(4.0, time),
    }
}

// ---- Textures and sprites ----

/// A skin texture ready to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinSprite {
    pub texture: TextureId,
    pub uv: UvRect,
    /// Display size in osu!pixels at scale 1: pixels divided by the texture's scale (2 for
    /// `@2x`), cropped around the centre to a maximum.
    pub size: Vec2,
}

impl SkinSprite {
    /// The sprite of an uploaded image whose file has `texture_scale` (2 for `@2x`), cropped
    /// around its centre to `max_size` × `max_size` display units.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs (WithMaximumSize)
    pub fn new(region: AtlasRegion, texture_scale: f32, max_size: f32) -> Self {
        Self::within(region, texture_scale, Vec2::new(max_size, max_size))
    }

    /// [`Self::new`] cropped to `max_size.x` × `max_size.y` display units.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs (WithMaximumSize)
    pub fn within(region: AtlasRegion, texture_scale: f32, max_size: Vec2) -> Self {
        let full = Vec2::new(
            region.size.width as f32 / texture_scale,
            region.size.height as f32 / texture_scale,
        );
        let size = Vec2::new(full.x.min(max_size.x), full.y.min(max_size.y));
        let crop = |u0: f32, u1: f32, keep: f32| {
            let margin = (u1 - u0) * (1.0 - keep) / 2.0;
            (u0 + margin, u1 - margin)
        };
        let (u0, u1) = crop(region.uv.u0, region.uv.u1, size.x / full.x);
        let (v0, v1) = crop(region.uv.v0, region.uv.v1, size.y / full.y);
        SkinSprite {
            texture: region.texture,
            uv: UvRect { u0, v0, u1, v1 },
            size,
        }
    }

    /// [`Self::new`] for a circle, overlay or approach circle texture.
    pub fn circle(region: AtlasRegion, texture_scale: f32) -> Self {
        Self::new(region, texture_scale, MAX_CIRCLE_SIZE)
    }

    /// [`Self::new`] for a combo number digit.
    pub fn digit(region: AtlasRegion, texture_scale: f32) -> Self {
        Self::new(region, texture_scale, MAX_DIGIT_SIZE)
    }
}

/// The skin textures of hit circles for one frame; missing ones are not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CircleTextures {
    pub circle: Option<SkinSprite>,
    pub overlay: Option<SkinSprite>,
    pub approach: Option<SkinSprite>,
    /// `{HitCirclePrefix}-0` to `-9`; without `-0` no numbers are drawn (`HasFont`).
    pub digits: [Option<SkinSprite>; 10],
}

/// The skin configuration of hit circles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircleSkinConfig {
    /// `HitCircleOverlayAboveNumber`.
    pub overlay_above_number: bool,
    /// `HitCircleOverlap`: positive values bring digits closer together.
    pub overlap: f32,
}

impl CircleSkinConfig {
    /// The configuration lazer reads through the skin source: the first skin that sets each key.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyMainCirclePiece.cs (load)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs (GetFontOverlap)
    pub fn new(skins: &SkinChain) -> Self {
        CircleSkinConfig {
            overlay_above_number: skins
                .config(|s| s.setting_bool("HitCircleOverlayAboveNumber"))
                .is_none_or(|f| f.value),
            overlap: skins
                .config(|s| s.setting_f32("HitCircleOverlap"))
                .map_or(-2.0, |f| f.value),
        }
    }
}

/// Multiplies a colour by a brightness and an alpha.
pub(crate) fn tint(colour: Color, brightness: f32, alpha: f32) -> Color {
    Color {
        r: colour.r * brightness,
        g: colour.g * brightness,
        b: colour.b * brightness,
        a: colour.a * alpha,
    }
}

/// A sprite of `size` window units centred on `centre`.
pub(crate) fn centred(texture: &SkinSprite, centre: Vec2, size: Vec2, colour: Color) -> Sprite {
    Sprite {
        position: [centre.x, centre.y],
        size: [size.x, size.y],
        origin: [size.x / 2.0, size.y / 2.0],
        rotation: 0.0,
        colour,
        texture: texture.texture,
        uv: texture.uv,
        blend: Blend::Alpha,
    }
}

/// Writes the combo number `number` centred on `centre`, each digit `scale` window units per
/// display unit.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs (HitCircleText)
fn push_number(
    batch: &mut SpriteBatch,
    textures: &CircleTextures,
    config: &CircleSkinConfig,
    number: u32,
    centre: Vec2,
    scale: f32,
    colour: Color,
) {
    if textures.digits[0].is_none() {
        return;
    }
    let font = LegacyFont {
        digits: &textures.digits,
        overlap: config.overlap,
    };
    let centred = Vec2::new(0.5, 0.5);
    font.push(
        batch,
        &Digits::new(u64::from(number)),
        centre,
        centred,
        scale,
        colour,
    );
}

/// Everything the draw side keeps to draw the hit circles of a play.
#[derive(Debug, Clone)]
pub struct HitCircles {
    pub visuals: CircleVisuals,
    pub style: CircleStyle,
    pub config: CircleSkinConfig,
}

impl HitCircles {
    /// Writes the sprites of the `live` circles at game `time`: circle pieces with earlier
    /// circles on top, then every approach circle above them in start time order, as lazer's
    /// playfield proxies approach circles into a layer above all objects.
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        live: &[LiveCircle],
        time: f64,
        textures: &CircleTextures,
    ) {
        push_circles(self, batch, layout, live, time, textures);
    }
}

fn push_circles(
    circles: &HitCircles,
    batch: &mut SpriteBatch,
    layout: &PlayfieldLayout,
    live: &[LiveCircle],
    time: f64,
    textures: &CircleTextures,
) {
    for c in live.iter().rev() {
        if let Some(circle) = circles.visuals.get(c.index) {
            let pose = circle_pose(circle, c.judgement, time, &circles.style);
            let all = PieceLayers::All;
            circles.push_piece(
                batch,
                layout,
                circle,
                &pose,
                1.0,
                textures.into(),
                textures,
                all,
            );
        }
    }
    let Some(approach) = &textures.approach else {
        return;
    };
    for c in live {
        if let Some(circle) = circles.visuals.get(c.index) {
            let pose = circle_pose(circle, c.judgement, time, &circles.style);
            push_approach(batch, layout, circle, &pose, 1.0, approach);
        }
    }
}

/// The circle and overlay textures of a circle piece: a hit circle's, or a slider head's.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PieceTextures {
    pub circle: Option<SkinSprite>,
    pub overlay: Option<SkinSprite>,
}

impl From<&CircleTextures> for PieceTextures {
    fn from(textures: &CircleTextures) -> Self {
        PieceTextures {
            circle: textures.circle,
            overlay: textures.overlay,
        }
    }
}

/// Which layers of a circle piece to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PieceLayers {
    All,
    /// The circle sprite only.
    Circle,
    /// The overlay and the combo number only: `LegacyMainCirclePiece.OverlayLayer`, which a
    /// slider head proxies above its reverse arrows.
    Overlay,
}

impl HitCircles {
    /// Writes the `layers` of the circle piece of `circle` in `pose` (its circle, overlay and
    /// combo number), within a parent of `parent_alpha` (a slider, for its head). Digits come
    /// from `textures`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_piece(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        circle: &CircleVisual,
        pose: &CirclePose,
        parent_alpha: f32,
        piece: PieceTextures,
        textures: &CircleTextures,
        layers: PieceLayers,
    ) {
        let alpha = parent_alpha * pose.object_alpha * pose.piece_alpha;
        if alpha <= 0.0 {
            return;
        }
        let centre = layout.to_window(circle.position);
        let object_scale = circle.scale * layout.scale();
        let white = Color::WHITE;
        let sprite_scale = object_scale * pose.sprite_scale;
        let sprite_alpha = alpha * pose.sprite_alpha;

        if layers != PieceLayers::Overlay
            && let Some(t) = &piece.circle
        {
            let size = Vec2::new(t.size.x * sprite_scale, t.size.y * sprite_scale);
            let colour = tint(circle.colour, pose.brightness, sprite_alpha);
            batch.push(&centred(t, centre, size, colour));
        }
        if layers == PieceLayers::Circle {
            return;
        }
        let overlay = |batch: &mut SpriteBatch| {
            if let Some(t) = &piece.overlay {
                let size = Vec2::new(t.size.x * sprite_scale, t.size.y * sprite_scale);
                let colour = tint(white, pose.brightness, sprite_alpha);
                batch.push(&centred(t, centre, size, colour));
            }
        };
        let number = |batch: &mut SpriteBatch| {
            let scale = object_scale * NUMBER_SCALE * pose.number_scale;
            let colour = tint(white, pose.brightness, alpha * pose.number_alpha);
            if colour.a > 0.0 {
                push_number(
                    batch,
                    textures,
                    &self.config,
                    circle.number,
                    centre,
                    scale,
                    colour,
                );
            }
        };
        if self.config.overlay_above_number {
            number(batch);
            overlay(batch);
        } else {
            overlay(batch);
            number(batch);
        }
    }
}

/// Writes the approach circle of `circle` in `pose`, within a parent of `parent_alpha`.
pub(crate) fn push_approach(
    batch: &mut SpriteBatch,
    layout: &PlayfieldLayout,
    circle: &CircleVisual,
    pose: &CirclePose,
    parent_alpha: f32,
    approach: &SkinSprite,
) {
    let alpha = parent_alpha * pose.object_alpha * pose.approach_alpha;
    if alpha <= 0.0 {
        return;
    }
    // `LegacyApproachCircle` is a sprite of its texture's size.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyApproachCircle.cs
    let scale = circle.scale * layout.scale() * pose.approach_scale;
    let size = Vec2::new(approach.size.x * scale, approach.size.y * scale);
    let colour = tint(circle.colour, 1.0, alpha);
    batch.push(&centred(
        approach,
        layout.to_window(circle.position),
        size,
        colour,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settling_a_track_keeps_its_values() {
        let mut track = Track::new(Kind::Scale);
        track.add(0.0, 100.0, 2.0, Easing::Out);
        track.add(50.0, 0.0, 2.2, Easing::None);
        track.add(50.0, 200.0, 2.0, Easing::None);
        track.add(300.0, 100.0, 4.0, Easing::None);
        let times = [50.0, 60.0, 250.0, 299.0, 300.0, 350.0, 500.0];
        let before: Vec<f32> = times.iter().map(|&t| track.value(1.0, t)).collect();
        let mut initial = 1.0;
        track.settle(&mut initial, 50.0);
        // The transform started last by then and the one not started yet are kept.
        assert_eq!(track.len, 2);
        let after: Vec<f32> = times.iter().map(|&t| track.value(initial, t)).collect();
        assert_eq!(before, after);
    }

    const START: f64 = 1000.0;
    /// AR 9.
    const PREEMPT: f64 = 600.0;
    const APPEAR: f64 = START - PREEMPT;
    /// OD 5: windows 99.5 and 149.5 ms.
    const OK: f64 = 99.5;
    const MEH: f64 = 149.5;

    fn circle(fade_in: f64) -> CircleVisual {
        CircleVisual {
            start_time: START,
            position: Vec2::new(256.0, 192.0),
            scale: 0.5,
            time_preempt: PREEMPT,
            time_fade_in: fade_in,
            colour: Color::WHITE,
            number: 1,
            increased_visibility: false,
        }
    }

    fn style(hidden: Option<bool>, fade_early: bool) -> CircleStyle {
        CircleStyle {
            mods: CircleMods {
                hidden,
                traceable: false,
                fade_early,
            },
            ok_window: OK,
            meh_window: MEH,
            number_short_fade: true,
        }
    }

    fn judged(time: f64, result: HitResult) -> Option<CircleJudgement> {
        Some(CircleJudgement { time, result })
    }

    fn pose_at(time: f64) -> CirclePose {
        circle_pose(&circle(400.0), None, time, &style(None, false))
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-5,
            "{actual} is not {expected}"
        );
    }

    #[test]
    fn fades_in_and_approaches_over_the_preempt() {
        let p = pose_at(APPEAR);
        assert_eq!((p.piece_alpha, p.approach_alpha), (0.0, 0.0));
        assert_eq!(p.approach_scale, 4.0);
        assert_eq!(p.brightness, DIM);
        assert_eq!(p.object_alpha, 1.0);

        // A third of the preempt: half of the 400 ms fade-in; the approach circle fades to 0.9
        // over twice the fade-in, capped at the preempt.
        let p = pose_at(APPEAR + 200.0);
        assert_close(p.piece_alpha, 0.5);
        assert_close(p.approach_alpha, 0.3);
        assert_close(p.approach_scale, 3.0);

        // Undimmed over 100 ms from 400 ms before the start time.
        assert_eq!(pose_at(START - 400.0).brightness, DIM);
        // Halfway in linear space.
        let halfway = to_srgb((to_linear(DIM) + 1.0) / 2.0);
        assert_close(pose_at(START - 350.0).brightness, halfway);
        assert!((halfway - 0.893).abs() < 1e-3);
        assert_eq!(pose_at(START - 300.0).brightness, 1.0);

        let p = pose_at(START);
        assert_eq!(p.piece_alpha, 1.0);
        assert_close(p.approach_alpha, 0.9);
        assert_eq!(p.approach_scale, 1.0);
    }

    #[test]
    fn approach_circle_fades_at_the_start_time() {
        assert_close(pose_at(START + 25.0).approach_alpha, 0.45);
        assert_eq!(pose_at(START + 50.0).approach_alpha, 0.0);
        // An unjudged circle stays visible.
        assert_eq!(pose_at(START + 100.0).object_alpha, 1.0);
    }

    #[test]
    fn hit_scales_and_fades_the_circle() {
        let s = style(None, false);
        let at = |t| {
            circle_pose(
                &circle(400.0),
                judged(START + 10.0, HitResult::Great),
                t,
                &s,
            )
        };
        // The approach circle disappears at once.
        assert_eq!(at(START + 10.0).approach_alpha, 0.0);
        let p = at(START + 130.0);
        assert_close(p.sprite_alpha, 0.5);
        // Easing.Out at half time: 0.75 of the way.
        assert_close(p.sprite_scale, 1.3);
        // A new skin only fades the number, over 60 ms.
        assert_eq!(p.number_alpha, 0.0);
        assert_close(at(START + 40.0).number_alpha, 0.5);
        assert_eq!(p.number_scale, 1.0);
        assert_eq!(at(START + 250.0).sprite_alpha, 0.0);
        assert_close(at(START + 250.0).sprite_scale, 1.4);
        // The drawable itself goes 800 ms after the hit.
        assert_eq!(at(START + 809.0).object_alpha, 1.0);
        assert_eq!(at(START + 810.0).object_alpha, 0.0);
    }

    #[test]
    fn old_skins_scale_the_number_with_the_circle() {
        let s = CircleStyle {
            number_short_fade: false,
            ..style(None, false)
        };
        let p = circle_pose(
            &circle(400.0),
            judged(START, HitResult::Ok),
            START + 120.0,
            &s,
        );
        assert_close(p.number_alpha, 0.5);
        assert_close(p.number_scale, 1.3);
    }

    #[test]
    fn miss_fades_the_whole_circle() {
        let s = style(None, false);
        let miss = START + 150.0;
        let at = |t| circle_pose(&circle(400.0), judged(miss, HitResult::Miss), t, &s);
        assert_eq!(at(miss).object_alpha, 1.0);
        assert_close(at(miss + 50.0).object_alpha, 0.5);
        assert_eq!(at(miss + 100.0).object_alpha, 0.0);
        // Nothing else animates on a miss.
        assert_eq!(at(miss + 50.0).sprite_scale, 1.0);
    }

    #[test]
    fn hidden_fades_circles_out_and_hides_approach_circles() {
        // Hidden's fade-in: 0.4 of the preempt.
        let fade_in = PREEMPT * 0.4;
        let s = style(Some(false), false);
        let at = |t| circle_pose(&circle(fade_in), None, t, &s);
        assert_close(at(APPEAR + 120.0).piece_alpha, 0.5);
        // Fades out over 0.3 of the preempt once faded in.
        let fade_start = APPEAR + fade_in;
        assert_eq!(at(fade_start).object_alpha, 1.0);
        assert_close(at(fade_start + 90.0).object_alpha, 0.5);
        assert_eq!(at(fade_start + 180.0).object_alpha, 0.0);
        for t in [APPEAR, APPEAR + 300.0, START] {
            assert_eq!(at(t).approach_alpha, 0.0);
        }
    }

    #[test]
    fn hidden_spares_the_first_object() {
        let fade_in = PREEMPT * 0.4;
        let first = CircleVisual {
            increased_visibility: true,
            ..circle(fade_in)
        };
        let s = style(Some(false), false);
        let at = |t| circle_pose(&first, None, t, &s);
        // The approach circle shows, and only the circle piece fades.
        assert_close(at(APPEAR + 240.0).approach_alpha, 0.45);
        let p = at(APPEAR + fade_in + 90.0);
        assert_eq!(p.object_alpha, 1.0);
        assert_close(p.piece_alpha, 0.5);
    }

    #[test]
    fn hidden_may_only_hide_approach_circles() {
        let s = style(Some(true), false);
        let p = circle_pose(&circle(240.0), None, START - 50.0, &s);
        assert_eq!((p.object_alpha, p.piece_alpha), (1.0, 1.0));
        assert_eq!(p.approach_alpha, 0.0);
    }

    #[test]
    fn classic_fades_into_the_miss() {
        let s = style(None, true);
        let idle = |t| circle_pose(&circle(400.0), None, t, &s);
        assert_eq!(idle(START + OK).object_alpha, 1.0);
        assert_close(idle(START + (OK + MEH) / 2.0).object_alpha, 0.5);
        // Invisible by the time the miss is judged.
        let miss = START + MEH + 1.0;
        let p = circle_pose(&circle(400.0), judged(miss, HitResult::Miss), miss, &s);
        assert_eq!(p.object_alpha, 0.0);
        // A late hit brings the circle back for its hit animation.
        let late = START + 140.0;
        let p = circle_pose(&circle(400.0), judged(late, HitResult::Meh), late, &s);
        assert_eq!(p.object_alpha, 1.0);
    }

    #[test]
    fn classic_early_fade_is_off_under_hidden() {
        let set = |mods: &[&str]| {
            ModSet::new(mods.iter().map(|m| Mod::from_acronym(m)).collect()).unwrap()
        };
        assert!(CircleMods::new(&set(&["CL"])).fade_early);
        assert!(!CircleMods::new(&set(&["CL", "HD"])).fade_early);
        let approach_only = Mod::Hidden {
            only_fade_approach_circles: true,
        };
        let mods = ModSet::new(vec![Mod::from_acronym("CL"), approach_only]).unwrap();
        assert_eq!(
            CircleMods::new(&mods),
            CircleMods {
                hidden: Some(true),
                traceable: false,
                fade_early: true
            }
        );
    }

    fn traceable_style() -> CircleStyle {
        CircleStyle {
            mods: CircleMods::new(&ModSet::new(vec![Mod::Traceable]).unwrap()),
            ..style(None, false)
        }
    }

    #[test]
    fn traceable_hides_the_circle_piece_but_not_the_approach_circle() {
        let s = traceable_style();
        assert!(s.mods.traceable && s.mods.adjusts_visibility());
        let at = |t| circle_pose(&circle(400.0), None, t, &s);
        for t in [APPEAR, APPEAR + 200.0, APPEAR + 400.0, START] {
            assert_eq!(at(t).piece_alpha, 0.0, "at {t}");
        }
        assert_close(at(APPEAR + 300.0).approach_alpha, 0.45);
        assert_eq!(at(APPEAR + 300.0).object_alpha, 1.0);
        // Nor does the hit animation show.
        let hit = START + 10.0;
        let p = circle_pose(
            &circle(400.0),
            judged(hit, HitResult::Great),
            hit + 50.0,
            &s,
        );
        assert_eq!(p.piece_alpha, 0.0);
    }

    #[test]
    fn traceable_spares_the_first_object() {
        let first = CircleVisual {
            increased_visibility: true,
            ..circle(400.0)
        };
        let p = circle_pose(&first, None, APPEAR + 400.0, &traceable_style());
        assert_eq!((p.object_alpha, p.piece_alpha), (1.0, 1.0));
        assert_close(p.approach_alpha, 0.6);
    }

    #[test]
    fn later_transforms_start_from_the_current_value() {
        let mut track = Track::new(Kind::Alpha);
        track.add(0.0, 200.0, 0.5, Easing::None);
        track.fade_out(100.0, 100.0);
        // The fade to 0.5 runs until the fade-out starts at 0.75, which takes over from there.
        assert_close(track.value(1.0, 50.0), 0.875);
        assert_close(track.value(1.0, 150.0), 0.375);
        assert_eq!(track.value(1.0, 200.0), 0.0);
        assert_eq!(track.value(1.0, -1.0), 1.0);
    }

    #[test]
    fn adding_a_transform_removes_the_ones_starting_after_it() {
        let mut track = Track::new(Kind::Alpha);
        track.fade_out(100.0, 100.0);
        track.add(0.0, 200.0, 0.5, Easing::None);
        assert_eq!(track.value(1.0, 300.0), 0.5);
    }

    #[test]
    fn a_preempt_under_400_ms_is_never_dimmed() {
        // AR 11: the undimming fade starts before the dim and removes it.
        let c = CircleVisual {
            time_preempt: 300.0,
            time_fade_in: 400.0 * 300.0 / 450.0,
            ..circle(400.0)
        };
        for t in [START - 300.0, START - 200.0, START] {
            assert_eq!(
                circle_pose(&c, None, t, &style(None, false)).brightness,
                1.0
            );
        }
    }

    #[test]
    fn an_early_hit_ends_the_approach_and_starts_the_fades_at_once() {
        let s = style(None, false);
        let hit = START - 80.0;
        let at = |t| circle_pose(&circle(400.0), judged(hit, HitResult::Ok), t, &s);
        assert_eq!(at(hit).approach_alpha, 0.0);
        assert_close(at(hit + 120.0).sprite_alpha, 0.5);
        assert_eq!(at(hit + 799.0).object_alpha, 1.0);
        assert_eq!(at(hit + 800.0).object_alpha, 0.0);
    }

    #[test]
    fn crops_large_textures_around_the_centre() {
        let region = AtlasRegion {
            texture: TextureId(1),
            origin: slam_render::Origin { x: 0, y: 0 },
            size: slam_render::Extent {
                width: 600,
                height: 200,
            },
            uv: UvRect {
                u0: 0.0,
                v0: 0.0,
                u1: 0.6,
                v1: 0.2,
            },
        };
        // 300 × 100 at @2x, cropped to 256 wide.
        let sprite = SkinSprite::circle(region, 2.0);
        assert_eq!((sprite.size.x, sprite.size.y), (256.0, 100.0));
        let margin = 0.6 * (1.0 - 256.0 / 300.0) / 2.0;
        assert_close(sprite.uv.u0, margin);
        assert_close(sprite.uv.u1, 0.6 - margin);
        assert_eq!((sprite.uv.v0, sprite.uv.v1), (0.0, 0.2));
    }
}
