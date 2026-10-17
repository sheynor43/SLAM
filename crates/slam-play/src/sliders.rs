//! Sliders as lazer draws them with a legacy skin: the body (ADR-0041), the head and tail circles,
//! ticks and repeats (their circles and reverse arrows), the ball and the follow circle.
//!
//! As for hit circles ([`crate::circles`]), the update side copies the sliders alive in
//! [`OsuRules`] into a [`LiveSliders`] snapshot and the draw side evaluates each one's transforms
//! at the frame's game time ([`slider_pose`]). Bodies are not sprites: [`push_objects`] writes
//! the sprites of circles and sliders in drawing order and records, for each body, the sprite it
//! goes before ([`SliderBodies`]); the caller draws the batch in parts with the bodies between.
//!
//! Not done: the body's `ShakeContainer` and the kiai flash of the head and tail pieces. Reverse
//! arrows keep the direction of the full path: they do not follow a body snaking out, and
//! their rotation is not smoothed over frames. Without frame history, the ball faces right where
//! lazer keeps its previous rotation (a path too short to tell the direction), and the follow
//! circle reads its scale for a tick pulse at the tick's hit state time rather than as of the
//! previous frame. Without a legacy ball (`sliderb`) or follow circle in the skins, none is drawn.

use slam_osu::objects::{NestedKind, OBJECT_RADIUS, OsuHitObjectKind, curve_position_at};
use slam_osu::rules::{CircleJudgement, LiveCircle, LiveSlider, OsuRules};
use slam_osu::{Beatmap, SliderPath, Vec2};
use slam_render::slider::{SliderGradient, SliderMesh};
use slam_render::sprite::{Affine, SpriteBatch};
use slam_skin::{Skin, SkinChain};

use crate::circles::{
    CircleMods, CirclePose, CircleSettings, CircleStyle, CircleTextures, CircleVisual, DIM, Easing,
    HitCircles, Kind, LEGACY_FADE_DURATION, MISS_WINDOW, PieceLayers, PieceTextures, SkinSprite,
    Track, centred, circle_pose, combo_colour, push_approach, tint, value_at,
};
use crate::fail::FailFrame;
use crate::playfield::PlayfieldLayout;
use crate::slider_body::{
    LegacyBodyColours, Snaking, SnakingTiming, body_mesh, range_distances, snaking_range,
};
use slam_osu::mods::HIDDEN_FADE_OUT_MULTIPLIER;
use slam_render::{Blend, Color};

use slam_skin::{SkinSource, SkinVersion};

/// The most sliders a [`LiveSliders`] snapshot holds and a frame draws; later ones are left out.
pub const MAX_LIVE_SLIDERS: usize = 128;

/// The most nested objects (heads, ticks, repeats, tails) of all sliders a [`LiveSliders`]
/// snapshot holds; the ticks and repeats of sliders that do not fit are not drawn.
pub const MAX_LIVE_NESTED: usize = 2048;

/// `DrawableSlider`'s `fade_out_time`.
const FADE_OUT_TIME: f64 = 240.0;

// ---- Snapshot ----

/// The sliders alive at an update, copied for the draw side without allocations.
/// Not `Copy`: it is large, copy it into a buffer slot with `clone_from`.
#[derive(Debug, Clone)]
pub struct LiveSliders {
    sliders: [LiveSlider; MAX_LIVE_SLIDERS],
    len: usize,
    /// Sliders left out because the snapshot was full.
    dropped: usize,
    /// The judgements of the nested objects of every slider, one run per slider.
    nested: [Option<CircleJudgement>; MAX_LIVE_NESTED],
    nested_len: usize,
    /// Each slider's run in `nested`: start and length; empty when it did not fit.
    runs: [(u32, u32); MAX_LIVE_SLIDERS],
    /// Sliders whose nested objects did not fit.
    nested_dropped: usize,
}

impl Default for LiveSliders {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveSliders {
    /// An empty snapshot.
    pub const fn new() -> Self {
        LiveSliders {
            sliders: [LiveSlider {
                index: 0,
                tracking: false,
                press_time: None,
                head: None,
                tail: None,
                judgement: None,
            }; MAX_LIVE_SLIDERS],
            len: 0,
            dropped: 0,
            nested: [None; MAX_LIVE_NESTED],
            nested_len: 0,
            runs: [(0, 0); MAX_LIVE_SLIDERS],
            nested_dropped: 0,
        }
    }

    /// Replaces the contents with the sliders alive at the last update of `rules`.
    pub fn capture(&mut self, rules: &OsuRules) {
        self.clear();
        for slider in rules.live_sliders() {
            self.push(slider, rules.nested_judgements(slider.index));
        }
    }

    /// Appends `slider` with the judgements of its nested objects in the beatmap's order;
    /// counted as dropped if full.
    pub fn push(
        &mut self,
        slider: LiveSlider,
        nested: impl IntoIterator<Item = Option<CircleJudgement>>,
    ) {
        if self.len == MAX_LIVE_SLIDERS {
            self.dropped += 1;
            return;
        }
        let start = self.nested_len;
        for judgement in nested {
            if self.nested_len == MAX_LIVE_NESTED {
                self.nested_len = start;
                self.nested_dropped += 1;
                break;
            }
            self.nested[self.nested_len] = judgement;
            self.nested_len += 1;
        }
        // Both fit in `u32`: at most `MAX_LIVE_NESTED`.
        self.runs[self.len] = (start as u32, (self.nested_len - start) as u32);
        self.sliders[self.len] = slider;
        self.len += 1;
    }

    /// Empties the snapshot.
    pub fn clear(&mut self) {
        self.len = 0;
        self.dropped = 0;
        self.nested_len = 0;
        self.nested_dropped = 0;
    }

    /// The judgements of the nested objects of slider `i` of [`Self::as_slice`], in the
    /// beatmap's order; empty if they did not fit.
    pub fn nested(&self, i: usize) -> &[Option<CircleJudgement>] {
        let (start, len) = self.runs[..self.len][i];
        &self.nested[start as usize..(start + len) as usize]
    }

    /// How many sliders' nested objects did not fit at the last capture.
    pub fn nested_dropped(&self) -> usize {
        self.nested_dropped
    }

    /// The sliders, in start time order.
    pub fn as_slice(&self) -> &[LiveSlider] {
        &self.sliders[..self.len]
    }

    /// How many alive sliders did not fit at the last capture.
    pub fn dropped(&self) -> usize {
        self.dropped
    }
}

// ---- Static data ----

/// A slider tail's static drawing data (lazer's `SliderTailCircle`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TailVisual {
    pub start_time: f64,
    /// Stacked position, in osu!pixels.
    pub position: Vec2,
    pub time_preempt: f64,
    pub time_fade_in: f64,
    /// The index of the span the tail ends.
    pub repeat_index: i32,
}

/// What a nested object of a slider draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NestedPiece {
    /// Drawn by the slider itself ([`SliderVisual::head`]).
    Head,
    /// A tick: `sliderscorepoint`.
    Tick,
    /// A repeat: an end circle and a reverse arrow.
    Repeat {
        /// The index of the span the repeat ends.
        repeat_index: i32,
        /// The arrow's direction in degrees, clockwise from +x.
        rotation: f32,
    },
    /// Drawn by the slider itself ([`SliderVisual::tail`]).
    Tail,
}

/// A nested object's static drawing data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NestedVisual {
    pub piece: NestedPiece,
    pub start_time: f64,
    /// Stacked position, in osu!pixels.
    pub position: Vec2,
    pub time_preempt: f64,
    pub time_fade_in: f64,
}

/// One slider's static drawing data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderVisual {
    /// The head circle, drawn as a hit circle at the slider's position.
    pub head: CircleVisual,
    pub tail: TailVisual,
    pub end_time: f64,
    /// The slider's timing; its preempt is the slider's.
    pub timing: SnakingTiming,
    /// The slider's own fade-in (Hidden leaves it alone), for the body.
    pub time_fade_in: f64,
    /// `Slider.SpanDuration`.
    pub span_duration: f64,
    /// Length of the path, in osu!pixels.
    pub distance: f64,
    /// Index of the path in [`SliderVisuals::path`].
    pub path: u32,
    /// `Slider.SpanCount`.
    pub span_count: i32,
    /// `Slider.Velocity`, in osu!pixels per millisecond.
    pub velocity: f64,
    /// `PlaySliderBody.PathRadius`: `OBJECT_RADIUS * Scale`, in osu!pixels.
    pub radius: f32,
    /// Index of the body's gradient in [`SliderVisuals::gradient`].
    pub gradient: usize,
    /// The slider's run of nested objects in [`SliderVisuals::nested`]: start and length.
    pub nested: (u32, u32),
}

/// The static drawing data of every slider of a beatmap, by object index, with the body
/// gradients of its colours.
#[derive(Debug, Clone)]
pub struct SliderVisuals {
    sliders: Vec<Option<SliderVisual>>,
    gradients: Vec<SliderGradient>,
    nested: Vec<NestedVisual>,
    paths: Vec<SliderPath>,
}

impl SliderVisuals {
    /// The sliders of `beatmap` (with its mods applied), coloured from `beatmap_skin` (the
    /// beatmap's own [`Skin::load_beatmap`], if any) and `skins`, as [`crate::circles`]
    /// colours circles.
    pub fn new(
        beatmap: &Beatmap,
        mods: CircleMods,
        settings: CircleSettings,
        beatmap_skin: Option<&Skin>,
        skins: &SkinChain,
    ) -> Self {
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModWithVisibilityAdjustment.cs (ApplyToDrawableHitObject, isObjectEqualToOrNestedIn)
        let first = beatmap.hit_objects.iter().position(|o| !o.is_spinner());
        let increase_first = mods.hidden.is_some() && settings.increase_first_object_visibility;
        let beatmap_colours =
            beatmap_skin.filter(|s| settings.beatmap_colours && !s.ini().combo_colours.is_empty());
        let mut gradients: Vec<SliderGradient> = Vec::new();
        let mut gradient_colours: Vec<[f32; 4]> = Vec::new();
        let mut nested: Vec<NestedVisual> = Vec::new();
        let mut curve: Vec<Vec2> = Vec::new();
        let mut paths: Vec<SliderPath> = Vec::new();
        let sliders = beatmap
            .hit_objects
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let OsuHitObjectKind::Slider(slider) = &o.kind else {
                    return None;
                };
                let head = slider
                    .nested
                    .iter()
                    .find(|n| matches!(n.kind, NestedKind::Head))?;
                let tail = slider.nested.iter().find_map(|n| match n.kind {
                    NestedKind::Tail { repeat_index } => Some((n, repeat_index)),
                    _ => None,
                })?;
                let colour = combo_colour(&o.combo, beatmap_colours, skins);
                let radius = OBJECT_RADIUS * o.defaults.scale;
                let accent = [colour.r, colour.g, colour.b, colour.a];
                let gradient = match gradient_colours.iter().position(|c| *c == accent) {
                    Some(g) => g,
                    None => {
                        let colours = LegacyBodyColours::new(accent, beatmap_skin, skins);
                        gradients.push(colours.gradient(radius));
                        gradient_colours.push(accent);
                        gradients.len() - 1
                    }
                };
                let position = o.stacked_position();
                let (tail, repeat_index) = tail;
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderTail.cs (OnApply)
                let tail_end = if repeat_index % 2 == 0 { 1.0 } else { 0.0 };
                let nested_start = nested.len();
                paths.push(slider.path.clone());
                nested.extend(slider.nested.iter().map(|n| NestedVisual {
                    piece: match n.kind {
                        NestedKind::Head => NestedPiece::Head,
                        NestedKind::Tick { .. } => NestedPiece::Tick,
                        NestedKind::Repeat { repeat_index, .. } => NestedPiece::Repeat {
                            repeat_index,
                            rotation: arrow_rotation(
                                &slider.path,
                                repeat_index % 2 == 0,
                                &mut curve,
                            ),
                        },
                        NestedKind::Tail { .. } => NestedPiece::Tail,
                    },
                    start_time: n.start_time,
                    position: n.stacked_position(),
                    time_preempt: n.defaults.time_preempt,
                    time_fade_in: n.defaults.time_fade_in,
                }));
                Some(SliderVisual {
                    head: CircleVisual {
                        start_time: head.start_time,
                        position,
                        scale: o.defaults.scale,
                        time_preempt: head.defaults.time_preempt,
                        time_fade_in: head.defaults.time_fade_in,
                        colour,
                        number: u32::try_from(o.combo.index_in_current_combo + 1).unwrap_or(0),
                        increased_visibility: increase_first && first == Some(i),
                    },
                    tail: TailVisual {
                        start_time: tail.start_time,
                        position: position + slider.curve_position_at(tail_end),
                        time_preempt: tail.defaults.time_preempt,
                        time_fade_in: tail.defaults.time_fade_in,
                        repeat_index,
                    },
                    end_time: slider.end_time(o.start_time),
                    timing: SnakingTiming::new(slider, o.start_time, o.defaults.time_preempt),
                    time_fade_in: o.defaults.time_fade_in,
                    span_duration: slider.span_duration(o.start_time),
                    distance: slider.path.distance(),
                    // Slider counts fit in `u32`, as object indices do.
                    path: (paths.len() - 1) as u32,
                    span_count: slider.span_count(),
                    velocity: slider.velocity,
                    radius,
                    gradient,
                    // Nested object counts fit in `u32`, as `ObjectRef::nested` assumes.
                    nested: (nested_start as u32, (nested.len() - nested_start) as u32),
                })
            })
            .collect();
        SliderVisuals {
            sliders,
            gradients,
            nested,
            paths,
        }
    }

    /// The path of `slider`, relative to its position.
    pub fn path(&self, slider: &SliderVisual) -> &SliderPath {
        &self.paths[slider.path as usize]
    }

    /// The nested objects of `slider`, in the beatmap's order.
    pub fn nested(&self, slider: &SliderVisual) -> &[NestedVisual] {
        let (start, len) = slider.nested;
        &self.nested[start as usize..(start + len) as usize]
    }

    /// The slider with object index `index`; `None` for other objects.
    pub fn get(&self, index: u32) -> Option<&SliderVisual> {
        self.sliders.get(index as usize)?.as_ref()
    }

    /// Body gradient `index` of [`SliderVisual::gradient`].
    pub fn gradient(&self, index: usize) -> &SliderGradient {
        &self.gradients[index]
    }

    /// The body meshes of the sliders of `beatmap`, by object index, in path coordinates
    /// (osu!pixels from the slider's position). Built at load, uploaded by the draw side.
    pub fn meshes(beatmap: &Beatmap) -> Vec<(u32, SliderMesh)> {
        beatmap
            .hit_objects
            .iter()
            .enumerate()
            .filter_map(|(i, o)| match &o.kind {
                // Beatmap object counts fit in `u32`, as `ObjectRef::index` assumes.
                OsuHitObjectKind::Slider(slider) => Some((i as u32, body_mesh(&slider.path))),
                _ => None,
            })
            .collect()
    }
}

/// The direction of the reverse arrow of a repeat at the end of `path` (`at_end`) or at its
/// start, in degrees: towards the first point of the drawn curve that is not the arrow's
/// position. The curve is the whole path (the body done snaking in).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderRepeat.cs (UpdateSnakingPosition)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (UpdateAfterChildren)
fn arrow_rotation(path: &SliderPath, at_end: bool, curve: &mut Vec<Vec2>) -> f32 {
    path.path_to_progress(curve, 0.0, 1.0);
    let position = path.position_at(if at_end { 1.0 } else { 0.0 });
    if curve.len() < 2 {
        // The arrow keeps its initial rotation.
        return 0.0;
    }
    // `Precision.AlmostEquals` of two vectors.
    let almost_equal =
        |p: &Vec2| (p.x - position.x).abs() <= 1e-3 && (p.y - position.y).abs() <= 1e-3;
    let aim = if at_end {
        curve.iter().rev().find(|p| !almost_equal(p))
    } else {
        curve.iter().find(|p| !almost_equal(p))
    }
    .copied()
    .unwrap_or(Vec2::new(0.0, 0.0));
    (aim.y - position.y).atan2(aim.x - position.x).to_degrees()
}

// ---- Poses ----

/// A slider tail's appearance at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TailPose {
    /// The alpha of the tail drawable.
    pub object_alpha: f32,
    /// The alpha of its circle piece: the delayed fade-in.
    pub piece_alpha: f32,
    /// The brightness the tail is tinted with: dimmed with the slider.
    pub brightness: f32,
    /// The alpha and scale of the circle and overlay sprites: the hit animation.
    pub sprite_alpha: f32,
    pub sprite_scale: f32,
}

/// A slider's appearance at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderPose {
    /// The alpha of the whole slider, head and tail included.
    pub object_alpha: f32,
    pub body_alpha: f32,
    pub body_brightness: f32,
    /// The visible part of the body, as progress along the path ([`snaking_range`]).
    pub range: (f64, f64),
    pub head: CirclePose,
    pub tail: TailPose,
}

/// The appearance of `slider` at game `time` in the state `live`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (UpdateInitialTransforms, UpdateHitStateTransforms, UpdateAfterChildren)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderHead.cs
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderTail.cs (UpdateInitialTransforms, UpdateHitStateTransforms)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableOsuHitObject.cs (applyDim, applyDimToDrawableHitObject, ApplyRepeatFadeIn)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Drawables/DrawableHitObject.cs (UpdateState, HitStateUpdateTime)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyMainCirclePiece.cs (updateStateTransforms)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModHidden.cs (applyHiddenState, getFadeOutParameters)
pub fn slider_pose(
    slider: &SliderVisual,
    live: &LiveSlider,
    time: f64,
    style: &CircleStyle,
    snaking: Snaking,
) -> SliderPose {
    let preempt = slider.timing.time_preempt;
    let appear = slider.timing.start_time - preempt;
    // An unjudged slider's hit state time is its end time.
    let hit_time = live.judgement.map_or(slider.end_time, |j| j.time);
    let head_hit = live.head.is_some_and(|j| j.result.is_hit());
    let hidden_fading = style.mods.hidden == Some(false);

    // The body.
    let mut body = Track::new(Kind::Alpha);
    body.add(appear, slider.time_fade_in, 1.0, Easing::None);
    if live.judgement.is_some_and(|j| j.result.is_hit()) && head_hit && snaking.snaking_out {
        body.fade_out(hit_time, 40.0);
    }
    if hidden_fading {
        let fade_start = appear + slider.time_fade_in;
        body.add(fade_start, slider.end_time - fade_start, 0.0, Easing::Out);
    }
    // The body and the tail are the slider's dimmable pieces.
    let dim = |track: &mut Track, at: f64| {
        track.add(at, 0.0, DIM, Easing::None);
        track.add(at + (preempt - MISS_WINDOW), 100.0, 1.0, Easing::None);
    };
    let mut body_brightness = Track::new(Kind::Colour);
    dim(&mut body_brightness, appear);

    SliderPose {
        object_alpha: slider_alpha(slider, live, time),
        body_alpha: body.value(0.0, time),
        body_brightness: body_brightness.value(1.0, time),
        range: snaking_range(&slider.timing, time, head_hit, snaking),
        head: circle_pose(&slider.head, live.head, time, style),
        tail: tail_pose(slider, live, time, style, snaking, &dim),
    }
}

/// The alpha of the slider drawable at `time`: faded out from its hit state time (its end time
/// while unjudged), whatever the state.
fn slider_alpha(slider: &SliderVisual, live: &LiveSlider, time: f64) -> f32 {
    let hit_time = live.judgement.map_or(slider.end_time, |j| j.time);
    let mut object = Track::new(Kind::Alpha);
    object.fade_out(hit_time, FADE_OUT_TIME);
    object.value(1.0, time)
}

fn tail_pose(
    slider: &SliderVisual,
    live: &LiveSlider,
    time: f64,
    style: &CircleStyle,
    snaking: Snaking,
    dim: &impl Fn(&mut Track, f64),
) -> TailPose {
    let tail = &slider.tail;
    let appear = tail.start_time - tail.time_preempt;
    // A tail has no hit windows: unjudged, its hit state time is its start time.
    let hit_time = live.tail.map_or(tail.start_time, |j| j.time);
    let hit = live.tail.is_some_and(|j| j.result.is_hit());

    let mut object = Track::new(Kind::Alpha);
    object.add(appear, 0.0, 1.0, Easing::None);
    match live.tail {
        None => object.fade_out(hit_time + tail.time_preempt, 500.0),
        // Hit animations are on.
        Some(_) if hit => object.fade_out(hit_time + 800.0, 0.0),
        Some(_) => object.fade_out(hit_time, 100.0),
    }
    if style.mods.hidden == Some(false) {
        // The tail fades as the head does, even for the first object.
        let head = &slider.head;
        let fade_start = head.start_time - head.time_preempt + head.time_fade_in;
        object.fade_out(fade_start, head.time_preempt * HIDDEN_FADE_OUT_MULTIPLIER);
    }

    // The first end circle waits for snaking in to finish.
    let mut piece = Track::new(Kind::Alpha);
    piece.fade_out(appear, 0.0);
    let delay = if snaking.snaking_in && tail.repeat_index == 0 {
        slider.timing.time_preempt / 3.0
    } else {
        0.0
    };
    let fade_in = if tail.repeat_index > 0 {
        slider.span_duration.min(tail.time_fade_in)
    } else {
        tail.time_fade_in
    };
    piece.add(appear + delay, fade_in, 1.0, Easing::None);

    // Dimmed with the slider, and again when judged: the slider dims its tail on every state
    // change of the tail, from the time of the change. The slider's own judgement dims it again
    // from its appearance, which removes the later dim: white from then on.
    let mut brightness = Track::new(Kind::Colour);
    dim(
        &mut brightness,
        slider.timing.start_time - slider.timing.time_preempt,
    );
    if let Some(j) = live.tail
        && live.judgement.is_none_or(|s| time < s.time)
    {
        dim(&mut brightness, j.time);
    }

    let mut sprite_alpha = Track::new(Kind::Alpha);
    let mut sprite_scale = Track::new(Kind::Scale);
    if hit {
        sprite_alpha.fade_out(hit_time, LEGACY_FADE_DURATION);
        sprite_scale.add(hit_time, LEGACY_FADE_DURATION, 1.4, Easing::Out);
    }

    TailPose {
        // Not shown before its preempt.
        object_alpha: object.value(0.0, time),
        piece_alpha: piece.value(1.0, time),
        brightness: brightness.value(1.0, time),
        sprite_alpha: sprite_alpha.value(1.0, time),
        sprite_scale: sprite_scale.value(1.0, time),
    }
}

/// `DrawableSliderTick.ANIM_DURATION`.
const TICK_ANIM_DURATION: f64 = 150.0;

/// A tick's appearance at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickPose {
    pub alpha: f32,
    pub scale: f32,
}

/// The appearance of `tick` at game `time`, judged as `judgement`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderTick.cs (UpdateInitialTransforms, UpdateHitStateTransforms)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModHidden.cs (applyHiddenState, getFadeOutParameters)
pub fn tick_pose(
    tick: &NestedVisual,
    judgement: Option<CircleJudgement>,
    time: f64,
    style: &CircleStyle,
) -> TickPose {
    let appear = tick.start_time - tick.time_preempt;
    // Unjudged, the hit state time is the start time.
    let hit_time = judgement.map_or(tick.start_time, |j| j.time);

    let mut alpha = Track::new(Kind::Alpha);
    alpha.fade_out(appear, 0.0);
    alpha.add(appear, TICK_ANIM_DURATION, 1.0, Easing::None);
    let mut scale = Track::new(Kind::Scale);
    scale.add(appear, 0.0, 0.5, Easing::None);
    scale.add(
        appear,
        TICK_ANIM_DURATION * 4.0,
        1.0,
        Easing::OutElasticHalf,
    );
    match judgement {
        None => alpha.fade_out(hit_time + tick.time_preempt, 0.0),
        Some(j) => {
            alpha.add(hit_time, TICK_ANIM_DURATION, 0.0, Easing::OutQuint);
            if j.result.is_hit() {
                // `Scale * 1.5` is read when the state changes.
                let from = scale.value(1.0, hit_time);
                scale.add(hit_time, TICK_ANIM_DURATION, from * 1.5, Easing::Out);
            }
        }
    }
    if style.mods.hidden == Some(false) {
        let duration = (tick.time_preempt - TICK_ANIM_DURATION).min(1000.0);
        alpha.fade_out(tick.start_time - duration, duration);
    }

    TickPose {
        // Not shown before its preempt.
        alpha: alpha.value(0.0, time),
        scale: scale.value(1.0, time),
    }
}

/// How a skin draws reverse arrows.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ArrowStyle {
    /// Skins of version 1 rock the arrow as it pulses.
    pub rotate: bool,
    /// The arrow comes from the default skin: drawn black on bright combo colours.
    pub default_texture: bool,
}

impl ArrowStyle {
    /// The style of the reverse arrows drawn with `skins`.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyReverseArrow.cs (load)
    pub fn new(skins: &SkinChain) -> Self {
        let version = skins.config(|s| Some(s.version())).map(|f| f.value);
        ArrowStyle {
            rotate: version.is_some_and(|v| v <= SkinVersion::V1),
            default_texture: skins
                .texture("reversearrow")
                .is_some_and(|f| f.source == SkinSource::Default),
        }
    }

    /// The arrow's colour on a slider of `accent` colour.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyReverseArrow.cs (LoadComplete)
    pub fn colour(self, accent: Color) -> Color {
        if self.default_texture && accent.r + accent.g + accent.b > 600.0 / 255.0 {
            Color::BLACK
        } else {
            Color::WHITE
        }
    }
}

/// A repeat's appearance at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RepeatPose {
    /// The alpha of the repeat drawable, arrow included.
    pub object_alpha: f32,
    /// The alpha of its circle piece: the delayed fade-in.
    pub piece_alpha: f32,
    /// The alpha and scale of the circle and overlay sprites: the hit animation.
    pub sprite_alpha: f32,
    pub sprite_scale: f32,
    pub arrow_alpha: f32,
    pub arrow_scale: f32,
    /// Degrees clockwise from +x: the path's direction and the rocking.
    pub arrow_rotation: f32,
}

/// The appearance of `repeat` of `slider` at game `time`, judged as `judgement`. Its
/// brightness is the slider body's ([`SliderPose::body_brightness`]): the slider dims its
/// repeats with the body.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderRepeat.cs (UpdateInitialTransforms, UpdateHitStateTransforms)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableOsuHitObject.cs (ApplyRepeatFadeIn)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyReverseArrow.cs (Update)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyMainCirclePiece.cs (updateStateTransforms)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModHidden.cs (applyHiddenState, getFadeOutParameters)
#[allow(clippy::too_many_arguments)]
pub fn repeat_pose(
    slider: &SliderVisual,
    repeat: &NestedVisual,
    judgement: Option<CircleJudgement>,
    time: f64,
    style: &CircleStyle,
    snaking: Snaking,
    arrows: ArrowStyle,
) -> RepeatPose {
    let (repeat_index, rotation) = match repeat.piece {
        NestedPiece::Repeat {
            repeat_index,
            rotation,
        } => (repeat_index, rotation),
        _ => {
            debug_assert!(false, "not a repeat: {repeat:?}");
            (0, 0.0)
        }
    };
    let appear = repeat.start_time - repeat.time_preempt;
    // Unjudged, the hit state time is the start time.
    let hit_time = judgement.map_or(repeat.start_time, |j| j.time);
    let hit = judgement.is_some_and(|j| j.result.is_hit());
    let anim_duration = slider.span_duration.min(300.0);
    let hidden_fading = style.mods.hidden == Some(false);

    let mut object = Track::new(Kind::Alpha);
    match judgement {
        None => object.fade_out(hit_time + repeat.time_preempt, 0.0),
        Some(_) if hit => object.add(hit_time, anim_duration, 0.0, Easing::Out),
        Some(_) => object.fade_out(hit_time, anim_duration),
    }
    if hidden_fading {
        object.fade_out(hit_time, 0.0);
    }

    // The first repeat waits for snaking in to finish.
    let delay = if snaking.snaking_in && repeat_index == 0 {
        slider.timing.time_preempt / 3.0
    } else {
        0.0
    };
    let fade_in = |duration: f64| {
        if repeat_index > 0 {
            slider.span_duration.min(duration)
        } else {
            duration
        }
    };
    let mut piece = Track::new(Kind::Alpha);
    piece.fade_out(appear, 0.0);
    piece.add(
        appear + delay,
        fade_in(repeat.time_fade_in),
        1.0,
        Easing::None,
    );
    if hidden_fading {
        // Only the circle fades, as the head does; the arrow stays.
        let head = &slider.head;
        let fade_start = head.start_time - head.time_preempt + head.time_fade_in;
        piece.fade_out(fade_start, head.time_preempt * HIDDEN_FADE_OUT_MULTIPLIER);
    }
    let mut arrow = Track::new(Kind::Alpha);
    arrow.fade_out(appear, 0.0);
    arrow.add(appear + delay, fade_in(150.0), 1.0, Easing::None);

    let mut sprite_alpha = Track::new(Kind::Alpha);
    let mut sprite_scale = Track::new(Kind::Scale);
    if hit {
        sprite_alpha.fade_out(hit_time, LEGACY_FADE_DURATION);
        sprite_scale.add(hit_time, LEGACY_FADE_DURATION, 1.4, Easing::Out);
    }

    // The arrow pulses every 300 ms from the repeat's appearance; once hit, it grows instead and
    // keeps its last rocking.
    let pulse = |at: f64| {
        const DURATION: f64 = 300.0;
        const ROTATION: f32 = 5.625;
        let loop_time = (at - appear) % DURATION;
        if arrows.rotate {
            (
                value_at(loop_time, ROTATION, -ROTATION, 0.0, DURATION, Easing::None),
                value_at(loop_time, 1.3, 1.0, 0.0, DURATION, Easing::None),
            )
        } else {
            (
                0.0,
                value_at(loop_time, 1.3, 1.0, 0.0, DURATION, Easing::Out),
            )
        }
    };
    let (rocking, arrow_scale) = if hit && time >= hit_time {
        let end = hit_time + anim_duration;
        (
            pulse(hit_time).0,
            value_at(time, 1.0, 1.4, hit_time, end, Easing::Out),
        )
    } else {
        pulse(time)
    };

    RepeatPose {
        // Not shown before its preempt.
        object_alpha: if time < appear {
            0.0
        } else {
            object.value(1.0, time)
        },
        piece_alpha: piece.value(1.0, time),
        sprite_alpha: sprite_alpha.value(1.0, time),
        sprite_scale: sprite_scale.value(1.0, time),
        arrow_alpha: arrow.value(1.0, time),
        arrow_scale,
        arrow_rotation: rotation + rocking,
    }
}

// ---- Ball and follow circle ----

/// `OsuLegacySkinTransformer.MAX_FOLLOW_CIRCLE_AREA_SIZE`: the most the ball's layers and the
/// follow circle show of their textures, in display units.
pub(crate) const MAX_FOLLOW_CIRCLE_AREA_SIZE: f32 = OBJECT_RADIUS * 2.0 * 3.0;

/// `LegacyFollowCircle`: legacy follow circles are drawn at half their size.
const FOLLOW_CIRCLE_TEXTURE_SCALE: f32 = 0.5;

/// The colour of `sliderb-nd` (`LegacySliderBall`).
const BALL_ND_COLOUR: Color = Color::rgb(5.0 / 255.0, 5.0 / 255.0, 5.0 / 255.0);

/// The frame of an animation of `count` frames of `frame_length` milliseconds each shown at
/// `playback` milliseconds from its start.
// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Animations/AnimationClockComposite.cs (PlaybackPosition)
// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Animations/Animation.cs (AddFrame, updateFrameIndex)
pub fn animation_frame(playback: f64, frame_length: f64, count: usize, looping: bool) -> usize {
    if count < 2 {
        return 0;
    }
    // Each frame starts where the previous ends; the duration is their sum.
    let mut duration = 0.0;
    for _ in 0..count {
        duration += frame_length;
    }
    let mut position = playback;
    if looping {
        position %= duration;
    }
    let position = position.clamp(0.0, duration);
    let mut frame = 0;
    let mut end = frame_length;
    while frame < count - 1 && position >= end {
        frame += 1;
        end += frame_length;
    }
    frame
}

/// How a skin draws the slider ball (`LegacySliderBall`), from the skin that provides it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BallStyle {
    /// The `SliderBall` colour of the skin.
    pub colour: Color,
    /// `AllowSliderBallTint`: the ball takes the combo colour instead.
    pub tint: bool,
}

impl Default for BallStyle {
    fn default() -> Self {
        BallStyle {
            colour: Color::WHITE,
            tint: false,
        }
    }
}

impl BallStyle {
    /// The skin of `skins` that draws the slider ball: the first with `sliderb` or `sliderb0`.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs (SliderBall)
    pub fn provider(skins: &SkinChain) -> Option<SkinSource> {
        skins.find_provider(|s| s.texture("sliderb").is_some() || s.texture("sliderb0").is_some())
    }

    /// The ball's style with `skins`; the ball reads the configuration of its own skin only.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBall.cs (load, LoadComplete)
    pub fn new(skins: &SkinChain) -> Self {
        let Some(provider) = Self::provider(skins) else {
            return BallStyle::default();
        };
        let skin = skins.skin(provider);
        let c = skin.slider_ball_colour();
        BallStyle {
            colour: Color {
                r: f32::from(c.r) / 255.0,
                g: f32::from(c.g) / 255.0,
                b: f32::from(c.b) / 255.0,
                a: f32::from(c.a) / 255.0,
            },
            tint: skin.allow_slider_ball_tint(),
        }
    }

    /// The colour of the ball of a slider of `accent` colour.
    pub fn ball_colour(self, accent: Color) -> Color {
        if self.tint { accent } else { self.colour }
    }
}

/// A slider ball's appearance at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BallPose {
    /// Where the ball is, in osu!pixels.
    pub position: Vec2,
    /// The alpha of the ball's container, follow circle included (`DrawableSliderBall`).
    pub container_alpha: f32,
    /// The alpha of the ball's sprites (`LegacySliderBall`).
    pub ball_alpha: f32,
    /// The ball's rotation in degrees clockwise: along the path, facing where it goes.
    pub rotation: f32,
    /// Milliseconds since the slider's animations started: they share its clock.
    pub playback: f64,
    /// The time each frame of the ball's animation is shown: slower on slow sliders.
    pub frame_length: f64,
    pub follow_alpha: f32,
    pub follow_scale: f32,
}

/// The progress of `slider` at `time`, clamped to its span.
fn completion_progress(slider: &SliderVisual, time: f64) -> f64 {
    let duration = slider.end_time - slider.timing.start_time;
    ((time - slider.timing.start_time) / duration).clamp(0.0, 1.0)
}

/// The appearance of the ball of `slider`, whose path is `path`, at game `time` in the state
/// `live` with its nested objects judged as `nested` (in the beatmap's order, empty when they
/// did not fit in the snapshot: the follow circle then reacts to tracking only).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderBall.cs (UpdateProgress)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (UpdateStartTimeStateTransforms, Update)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBall.cs (onHitObjectApplied, updateStateTransforms)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Drawables/DrawableHitObject.cs (UpdateState, AnimationStartTime)
pub fn ball_pose(
    slider: &SliderVisual,
    path: &SliderPath,
    nested: &[NestedVisual],
    live: &LiveSlider,
    judgements: &[Option<CircleJudgement>],
    time: f64,
) -> BallPose {
    let start = slider.timing.start_time;
    let hit_time = live.judgement.map_or(slider.end_time, |j| j.time);
    let progress = completion_progress(slider, time);
    let at = |p: f64| curve_position_at(path, slider.span_count, p);

    // 0.1 / distance is the progress that moves the ball 0.1 osu!pixels.
    let check = 0.1 / slider.distance;
    let diff = at((1.0 - check).min(progress)) - at(1.0f64.min(progress + check));
    // Lazer keeps the previous rotation when the path is too short to tell (very short
    // sliders, the turn of a repeat); without frame history, that is the initial rotation.
    // Lazer's `LengthFast` approximates the square root; the exact length is used here.
    let length = (diff.x * diff.x + diff.y * diff.y).sqrt();
    let rotation = if slider.distance > 0.0 && length >= 0.01 {
        -90.0 + (-f64::from(diff.x).atan2(f64::from(diff.y)) * 180.0 / std::f64::consts::PI) as f32
    } else {
        0.0
    };

    let sixty = slam_skin::SIXTY_FRAME_TIME;
    let (follow_alpha, follow_scale) = follow_pose(slider, nested, live, judgements, time);
    BallPose {
        position: slider.head.position + at(progress),
        container_alpha: if time >= start { 1.0 } else { 0.0 },
        ball_alpha: if time >= start && time < hit_time {
            1.0
        } else {
            0.0
        },
        rotation,
        playback: time - (start - slider.timing.time_preempt),
        frame_length: (0.15 / slider.velocity * sixty).max(sixty),
        follow_alpha,
        follow_scale,
    }
}

/// What the follow circle reacts to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum FollowEvent {
    End,
    Tick,
    Break,
}

/// The alpha and scale of the follow circle of `slider` at `time`. The press is the last
/// time tracking started; a press resets the animation and drops the later transforms, so
/// earlier ones need no replay. Events are replayed in the order they happened, the press
/// first at the same time (the slider updates before its nested objects): a nested object's at
/// its judgement, a press at its update. Each starts its animation at its own time: the hit
/// state time of the nested object, the slider's end time for a hit tail (lazer's tail is
/// judged up to 36 ms earlier), the start time at the earliest for a press.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/FollowCircle.cs
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyFollowCircle.cs
fn follow_pose(
    slider: &SliderVisual,
    nested: &[NestedVisual],
    live: &LiveSlider,
    judgements: &[Option<CircleJudgement>],
    time: f64,
) -> (f32, f32) {
    let (mut alpha0, mut scale0) = (0.0f32, 1.0f32);
    let mut alpha = Track::new(Kind::Alpha);
    let mut scale = Track::new(Kind::Scale);
    let mut press = live.press_time;

    // (when it happened, when its animation starts, what).
    let events = nested
        .iter()
        .zip(judgements)
        .filter_map(|(n, j)| {
            let j = (*j)?;
            let hit = j.result.is_hit();
            let event = match n.piece {
                NestedPiece::Head => return None,
                // The slider's hit state time: its end time until it is judged after its tail.
                NestedPiece::Tail if hit => {
                    return Some((j.time, slider.end_time, FollowEvent::End));
                }
                NestedPiece::Tick | NestedPiece::Repeat { .. } if hit => FollowEvent::Tick,
                NestedPiece::Tail | NestedPiece::Tick | NestedPiece::Repeat { .. } => {
                    FollowEvent::Break
                }
            };
            Some((j.time, j.time, event))
        })
        .chain(std::iter::once((
            f64::INFINITY,
            f64::INFINITY,
            FollowEvent::End,
        )));

    for (happened, at, event) in events {
        if let Some(raw) = press
            && raw <= happened
        {
            press = None;
            let start = raw.max(slider.timing.start_time);
            alpha.settle(&mut alpha0, start);
            scale.settle(&mut scale0, start);
            // The slider is unjudged: its hit state time is its end time.
            let remaining = (slider.end_time - raw).max(0.0);
            scale.add(start, 0.0, 1.0, Easing::None);
            scale.add(start, remaining.min(180.0), 2.0, Easing::Out);
            alpha.add(start, 0.0, 0.0, Easing::None);
            alpha.add(start, remaining.min(60.0), 1.0, Easing::None);
        }
        if happened == f64::INFINITY || happened > time {
            break;
        }
        // An animation may start after it happened: only what has started by then settles.
        alpha.settle(&mut alpha0, happened.min(at));
        scale.settle(&mut scale0, happened.min(at));
        match event {
            FollowEvent::End => {
                scale.add(at, 200.0, 1.6, Easing::Out);
                alpha.add(at, 200.0, 0.0, Easing::In);
            }
            FollowEvent::Tick => {
                // The scale the drawable has when the tick is judged.
                if scale.value(scale0, at) >= 2.0 {
                    scale.add(at, 0.0, 2.2, Easing::None);
                    scale.add(at, 200.0, 2.0, Easing::None);
                }
            }
            FollowEvent::Break => {
                scale.add(at, 100.0, 4.0, Easing::None);
                alpha.add(at, 100.0, 0.0, Easing::None);
            }
        }
    }
    (alpha.value(alpha0, time), scale.value(scale0, time))
}

// ---- Drawing ----

/// The skin textures of slider pieces for one frame; missing ones are not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SliderTextures {
    /// `sliderstartcircle` and its overlay, or the hit circle's.
    pub head: PieceTextures,
    /// `sliderendcircle` and its overlay, or the hit circle's; repeats too.
    pub tail: PieceTextures,
    /// `sliderscorepoint`.
    pub tick: Option<SkinSprite>,
    /// `reversearrow`.
    pub arrow: Option<SkinSprite>,
    /// The frames of `sliderb` (`sliderb0`, `sliderb1`, ...); their length is the slider's.
    pub ball: AnimationSprites,
    /// `sliderb-nd` and `sliderb-spec` of the skin that provides the ball.
    pub ball_nd: Option<SkinSprite>,
    pub ball_spec: Option<SkinSprite>,
    /// The frames of `sliderfollowcircle`.
    pub follow: AnimationSprites,
}

/// The most frames an animation draws; later ones are left out.
pub const MAX_ANIMATION_FRAMES: usize = 64;

/// The frames of a looping skin animation for one frame of drawing; empty when it is missing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimationSprites {
    frames: [Option<SkinSprite>; MAX_ANIMATION_FRAMES],
    len: usize,
    /// The time each frame is shown, in milliseconds.
    pub frame_length: f64,
}

impl Default for AnimationSprites {
    fn default() -> Self {
        AnimationSprites {
            frames: [None; MAX_ANIMATION_FRAMES],
            len: 0,
            frame_length: slam_skin::SIXTY_FRAME_TIME,
        }
    }
}

impl AnimationSprites {
    /// `frames`, of `frame_length` milliseconds each; the ones past [`MAX_ANIMATION_FRAMES`]
    /// are left out.
    pub fn new(frames: impl IntoIterator<Item = SkinSprite>, frame_length: f64) -> Self {
        let mut animation = AnimationSprites {
            frame_length,
            ..AnimationSprites::default()
        };
        for (slot, frame) in animation.frames.iter_mut().zip(frames) {
            *slot = Some(frame);
            animation.len += 1;
        }
        animation
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The frame shown at `playback` milliseconds from the start when each is shown for
    /// `frame_length` milliseconds; `None` when there are no frames.
    pub fn frame_at(&self, playback: f64, frame_length: f64) -> Option<&SkinSprite> {
        let frame = animation_frame(playback, frame_length, self.len, true);
        self.frames[..self.len].get(frame)?.as_ref()
    }
}

/// One slider body to draw this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyDraw {
    /// Object index of the slider: which mesh.
    pub index: u32,
    /// The body goes before this sprite of the batch (the batch's length when recorded).
    pub before_sprite: u32,
    /// The slider's position in osu!pixels: the origin of its mesh.
    pub position: Vec2,
    /// Path radius in osu!pixels.
    pub radius: f32,
    /// Snaking range in path length.
    pub range: (f32, f32),
    /// Straight-alpha tint: the dim and the fades.
    pub tint: [f32; 4],
    /// Index of the gradient in [`SliderVisuals::gradient`].
    pub gradient: usize,
    /// Where the body goes in window pixels after it is placed (the fail animation's fall).
    pub transform: Affine,
}

/// The slider bodies of a frame, in drawing order. Holds [`MAX_LIVE_SLIDERS`] without
/// allocating.
#[derive(Debug, Clone)]
pub struct SliderBodies {
    draws: Vec<BodyDraw>,
}

impl Default for SliderBodies {
    fn default() -> Self {
        Self::new()
    }
}

impl SliderBodies {
    pub fn new() -> Self {
        SliderBodies {
            draws: Vec::with_capacity(MAX_LIVE_SLIDERS),
        }
    }

    pub fn clear(&mut self) {
        self.draws.clear();
    }

    pub fn as_slice(&self) -> &[BodyDraw] {
        &self.draws
    }

    fn push(&mut self, draw: BodyDraw) {
        // A snapshot holds at most `MAX_LIVE_SLIDERS`; never grow in a frame.
        if self.draws.len() < self.draws.capacity() {
            self.draws.push(draw);
        }
    }

    pub fn len(&self) -> usize {
        self.draws.len()
    }

    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }

    /// Moves the bodies from the `first`-th on by `map` and multiplies their alpha.
    pub fn transform_from(&mut self, first: usize, map: &Affine, alpha: f32) {
        for draw in self.draws.iter_mut().skip(first) {
            draw.transform = map.then_after(&draw.transform);
            draw.tint[3] *= alpha;
        }
    }
}

/// Everything the draw side keeps to draw the sliders of a play.
#[derive(Debug, Clone)]
pub struct HitSliders {
    pub visuals: SliderVisuals,
    pub snaking: Snaking,
    pub arrows: ArrowStyle,
    pub ball: BallStyle,
}

/// The textures of one frame for [`push_objects`].
#[derive(Debug, Clone, Copy)]
pub struct ObjectTextures<'a> {
    pub circles: &'a CircleTextures,
    pub sliders: &'a SliderTextures,
}

/// Writes the sprites of the `circles` and `sliders` alive at game `time` in lazer's
/// drawing order, and records the slider bodies between them in `bodies`. Objects starting
/// earlier are on top; within a slider, the body, the tail, the ticks, the repeats, the head's
/// circle, the reverse arrows, the head's overlay and number, then the follow circle and the
/// ball. The approach circles go above them with [`push_approaches`].
#[allow(clippy::too_many_arguments)]
pub fn push_objects(
    hit_circles: &HitCircles,
    hit_sliders: &HitSliders,
    batch: &mut SpriteBatch,
    bodies: &mut SliderBodies,
    layout: &PlayfieldLayout,
    circles: &[LiveCircle],
    live_sliders: &LiveSliders,
    time: f64,
    textures: ObjectTextures<'_>,
    fail: Option<&FailFrame>,
) {
    let style = &hit_circles.style;
    let snaking = hit_sliders.snaking;
    let visuals = &hit_sliders.visuals;
    let sliders = live_sliders.as_slice();

    // Later objects first: merge both lists by descending object index.
    let (mut c, mut s) = (circles.len(), sliders.len());
    while c > 0 || s > 0 {
        let take_circle = s == 0 || (c > 0 && circles[c - 1].index > sliders[s - 1].index);
        if take_circle {
            c -= 1;
            let live = &circles[c];
            if let Some(circle) = hit_circles.visuals.get(live.index) {
                let first = batch.len();
                let pose = circle_pose(circle, live.judgement, time, style);
                let piece = PieceTextures::from(textures.circles);
                let all = PieceLayers::All;
                let circles = textures.circles;
                hit_circles.push_piece(batch, layout, circle, &pose, 1.0, piece, circles, all);
                if let Some(fail) = fail {
                    let map = fail.drop(live.index, appear(circle), circle.position, layout);
                    batch.transform_from(first, &map, [1.0, 1.0, 1.0, fail.objects_alpha]);
                }
            }
        } else {
            s -= 1;
            let live = &sliders[s];
            if let Some(slider) = visuals.get(live.index) {
                let (first, first_body) = (batch.len(), bodies.len());
                let pose = slider_pose(slider, live, time, style, snaking);
                let nested = Nested {
                    visuals: visuals.nested(slider),
                    judgements: live_sliders.nested(s),
                };
                push_slider(
                    hit_circles,
                    hit_sliders,
                    batch,
                    bodies,
                    layout,
                    live,
                    slider,
                    &pose,
                    nested,
                    time,
                    textures,
                );
                if let Some(fail) = fail {
                    // Lazer's `DrawableSlider` pivots at its head (`OriginPosition` is the
                    // path's start).
                    let head = &slider.head;
                    let map = fail.drop(live.index, appear(head), head.position, layout);
                    let alpha = fail.objects_alpha;
                    batch.transform_from(first, &map, [1.0, 1.0, 1.0, alpha]);
                    bodies.transform_from(first_body, &map, alpha);
                }
            }
        }
    }
}

/// When an object becomes alive (lazer's `LifetimeStart` of an osu! object): its fade-in
/// starts a preempt before it.
fn appear(circle: &CircleVisual) -> f64 {
    circle.start_time - circle.time_preempt
}

/// Writes the approach circles of the live `circles` and `sliders`' heads at game `time`, in
/// start time order: lazer's playfield proxies them into a layer above all objects and
/// judgements.
#[allow(clippy::too_many_arguments)]
pub fn push_approaches(
    hit_circles: &HitCircles,
    hit_sliders: &HitSliders,
    batch: &mut SpriteBatch,
    layout: &PlayfieldLayout,
    circles: &[LiveCircle],
    sliders: &[LiveSlider],
    time: f64,
    textures: &CircleTextures,
    fail: Option<&FailFrame>,
) {
    let style = &hit_circles.style;
    let visuals = &hit_sliders.visuals;
    // Lazer's proxies draw the approach circles where their objects are.
    let drop = |batch: &mut SpriteBatch, first: usize, index: u32, circle: &CircleVisual| {
        if let Some(fail) = fail {
            let map = fail.drop(index, appear(circle), circle.position, layout);
            batch.transform_from(first, &map, [1.0, 1.0, 1.0, fail.objects_alpha]);
        }
    };
    let Some(approach) = &textures.approach else {
        return;
    };
    let (mut c, mut s) = (0, 0);
    while c < circles.len() || s < sliders.len() {
        let take_circle =
            s == sliders.len() || (c < circles.len() && circles[c].index < sliders[s].index);
        if take_circle {
            let live = &circles[c];
            c += 1;
            if let Some(circle) = hit_circles.visuals.get(live.index) {
                let first = batch.len();
                let pose = circle_pose(circle, live.judgement, time, style);
                push_approach(batch, layout, circle, &pose, 1.0, approach);
                drop(batch, first, live.index, circle);
            }
        } else {
            let live = &sliders[s];
            s += 1;
            if let Some(slider) = visuals.get(live.index) {
                let first = batch.len();
                let pose = circle_pose(&slider.head, live.head, time, style);
                let parent = slider_alpha(slider, live, time);
                push_approach(batch, layout, &slider.head, &pose, parent, approach);
                drop(batch, first, live.index, &slider.head);
            }
        }
    }
}

/// A slider's nested objects: static data and judgements, in the beatmap's order.
#[derive(Debug, Clone, Copy)]
struct Nested<'a> {
    visuals: &'a [NestedVisual],
    /// Empty when they did not fit in the snapshot: then ticks and repeats are not drawn.
    judgements: &'a [Option<CircleJudgement>],
}

impl<'a> Nested<'a> {
    /// The ticks or repeats (`repeats`) with their judgements.
    fn pieces(
        self,
        repeats: bool,
    ) -> impl Iterator<Item = (&'a NestedVisual, Option<CircleJudgement>)> {
        let complete = self.visuals.len() == self.judgements.len();
        self.visuals
            .iter()
            .zip(self.judgements)
            .filter(move |(v, _)| {
                complete
                    && match v.piece {
                        NestedPiece::Tick => !repeats,
                        NestedPiece::Repeat { .. } => repeats,
                        NestedPiece::Head | NestedPiece::Tail => false,
                    }
            })
            .map(|(v, j)| (v, *j))
    }
}

/// Writes an end circle (a tail or a repeat) of `slider` at `position`.
#[allow(clippy::too_many_arguments)]
fn push_end_circle(
    batch: &mut SpriteBatch,
    layout: &PlayfieldLayout,
    slider: &SliderVisual,
    position: Vec2,
    brightness: f32,
    alpha: f32,
    scale: f32,
    pieces: &PieceTextures,
) {
    if alpha <= 0.0 {
        return;
    }
    let centre = layout.to_window(position);
    let scale = slider.head.scale * layout.scale() * scale;
    if let Some(t) = &pieces.circle {
        let size = Vec2::new(t.size.x * scale, t.size.y * scale);
        let colour = tint(slider.head.colour, brightness, alpha);
        batch.push(&centred(t, centre, size, colour));
    }
    if let Some(t) = &pieces.overlay {
        let size = Vec2::new(t.size.x * scale, t.size.y * scale);
        let colour = tint(Color::WHITE, brightness, alpha);
        batch.push(&centred(t, centre, size, colour));
    }
}

#[allow(clippy::too_many_arguments)]
fn push_slider(
    hit_circles: &HitCircles,
    hit_sliders: &HitSliders,
    batch: &mut SpriteBatch,
    bodies: &mut SliderBodies,
    layout: &PlayfieldLayout,
    live: &LiveSlider,
    slider: &SliderVisual,
    pose: &SliderPose,
    nested: Nested<'_>,
    time: f64,
    textures: ObjectTextures<'_>,
) {
    if pose.object_alpha <= 0.0 {
        return;
    }
    let body_alpha = pose.object_alpha * pose.body_alpha;
    if body_alpha > 0.0 {
        let b = pose.body_brightness;
        bodies.push(BodyDraw {
            index: live.index,
            // Sprite counts fit in `u32`, as the batch's quad indices assume.
            before_sprite: batch.len() as u32,
            position: slider.head.position,
            radius: slider.radius,
            range: range_distances(slider.distance, pose.range),
            tint: [b, b, b, body_alpha],
            gradient: slider.gradient,
            transform: Affine::IDENTITY,
        });
    }

    let tail = &pose.tail;
    push_end_circle(
        batch,
        layout,
        slider,
        slider.tail.position,
        tail.brightness,
        pose.object_alpha * tail.object_alpha * tail.piece_alpha * tail.sprite_alpha,
        tail.sprite_scale,
        &textures.sliders.tail,
    );

    let style = &hit_circles.style;
    if let Some(t) = &textures.sliders.tick {
        for (tick, judgement) in nested.pieces(false) {
            let p = tick_pose(tick, judgement, time, style);
            let alpha = pose.object_alpha * p.alpha;
            if alpha <= 0.0 {
                continue;
            }
            let scale = slider.head.scale * layout.scale() * p.scale;
            let size = Vec2::new(t.size.x * scale, t.size.y * scale);
            let colour = tint(Color::WHITE, 1.0, alpha);
            batch.push(&centred(t, layout.to_window(tick.position), size, colour));
        }
    }
    let repeat_pose = |repeat: &NestedVisual, judgement| {
        let (snaking, arrows) = (hit_sliders.snaking, hit_sliders.arrows);
        repeat_pose(slider, repeat, judgement, time, style, snaking, arrows)
    };
    for (repeat, judgement) in nested.pieces(true) {
        let p = repeat_pose(repeat, judgement);
        push_end_circle(
            batch,
            layout,
            slider,
            repeat.position,
            pose.body_brightness,
            pose.object_alpha * p.object_alpha * p.piece_alpha * p.sprite_alpha,
            p.sprite_scale,
            &textures.sliders.tail,
        );
    }

    // The head's circle, the reverse arrows, then the head's overlay and number: the slider
    // proxies the head's overlay layer above the arrows in its overlay container.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderHeadHitCircle.cs
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyReverseArrow.cs (onHitObjectApplied)
    let head = |batch: &mut SpriteBatch, layers| {
        hit_circles.push_piece(
            batch,
            layout,
            &slider.head,
            &pose.head,
            pose.object_alpha,
            textures.sliders.head,
            textures.circles,
            layers,
        );
    };
    head(batch, PieceLayers::Circle);
    if let Some(t) = &textures.sliders.arrow {
        let accent = hit_sliders.arrows.colour(slider.head.colour);
        for (repeat, judgement) in nested.pieces(true) {
            let p = repeat_pose(repeat, judgement);
            let alpha = pose.object_alpha * p.object_alpha * p.arrow_alpha;
            if alpha <= 0.0 {
                continue;
            }
            let scale = slider.head.scale * layout.scale() * p.arrow_scale;
            let size = Vec2::new(t.size.x * scale, t.size.y * scale);
            let colour = tint(accent, pose.body_brightness, alpha);
            let mut sprite = centred(t, layout.to_window(repeat.position), size, colour);
            sprite.rotation = p.arrow_rotation.to_radians();
            batch.push(&sprite);
        }
    }
    head(batch, PieceLayers::Overlay);

    let ball = ball_pose(
        slider,
        hit_sliders.visuals.path(slider),
        nested.visuals,
        live,
        nested.judgements,
        time,
    );
    push_ball(
        batch,
        layout,
        slider,
        pose.object_alpha,
        &ball,
        hit_sliders.ball,
        textures.sliders,
    );
}

/// Writes the follow circle and the ball of `slider` posed as `ball` in a slider of
/// `object_alpha`: the follow circle, then `sliderb-nd`, the ball and `sliderb-spec`
/// (additive). Only the ball turns along the path.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderBall.cs (load)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBall.cs (load, UpdateAfterChildren)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyFollowCircle.cs
fn push_ball(
    batch: &mut SpriteBatch,
    layout: &PlayfieldLayout,
    slider: &SliderVisual,
    object_alpha: f32,
    ball: &BallPose,
    style: BallStyle,
    textures: &SliderTextures,
) {
    let alpha = object_alpha * ball.container_alpha;
    if alpha <= 0.0 {
        return;
    }
    let centre = layout.to_window(ball.position);
    let scale = slider.head.scale * layout.scale();
    let sized = |t: &SkinSprite, scale: f32| Vec2::new(t.size.x * scale, t.size.y * scale);

    let follow_alpha = alpha * ball.follow_alpha;
    if follow_alpha > 0.0
        && let Some(t) = textures
            .follow
            .frame_at(ball.playback, textures.follow.frame_length)
    {
        let size = sized(t, scale * FOLLOW_CIRCLE_TEXTURE_SCALE * ball.follow_scale);
        batch.push(&centred(
            t,
            centre,
            size,
            tint(Color::WHITE, 1.0, follow_alpha),
        ));
    }

    let ball_alpha = alpha * ball.ball_alpha;
    let Some(frame) = textures.ball.frame_at(ball.playback, ball.frame_length) else {
        return;
    };
    if ball_alpha <= 0.0 {
        return;
    }
    if let Some(t) = &textures.ball_nd {
        batch.push(&centred(
            t,
            centre,
            sized(t, scale),
            tint(BALL_ND_COLOUR, 1.0, ball_alpha),
        ));
    }
    let colour = style.ball_colour(slider.head.colour);
    let mut sprite = centred(
        frame,
        centre,
        sized(frame, scale),
        tint(colour, 1.0, ball_alpha),
    );
    sprite.rotation = ball.rotation.to_radians();
    batch.push(&sprite);
    if let Some(t) = &textures.ball_spec {
        let mut sprite = centred(
            t,
            centre,
            sized(t, scale),
            tint(Color::WHITE, 1.0, ball_alpha),
        );
        sprite.blend = Blend::Additive;
        batch.push(&sprite);
    }
}
