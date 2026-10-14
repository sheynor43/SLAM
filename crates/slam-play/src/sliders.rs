//! Sliders as lazer draws them with a legacy skin: the body (ADR-0041), the head circle and the
//! tail circle. Ball, follow circle, ticks and repeats are not drawn yet.
//!
//! As for hit circles ([`crate::circles`]), the update side copies the sliders alive in
//! [`OsuRules`] into a [`LiveSliders`] snapshot and the draw side evaluates each one's transforms
//! at the frame's game time ([`slider_pose`]). Bodies are not sprites: [`push_objects`] writes
//! the sprites of circles and sliders in drawing order and records, for each body, the sprite it
//! goes before ([`SliderBodies`]); the caller draws the batch in parts with the bodies between.
//!
//! Not done: the body's `ShakeContainer` and the kiai flash of the head and tail pieces.

use slam_osu::objects::{NestedKind, OBJECT_RADIUS, OsuHitObjectKind};
use slam_osu::rules::{LiveCircle, LiveSlider, OsuRules};
use slam_osu::{Beatmap, Vec2};
use slam_render::slider::{SliderGradient, SliderMesh};
use slam_render::sprite::SpriteBatch;
use slam_skin::{Skin, SkinChain};

use crate::circles::{
    CircleMods, CirclePose, CircleSettings, CircleStyle, CircleTextures, CircleVisual, DIM, Easing,
    HitCircles, Kind, LEGACY_FADE_DURATION, MISS_WINDOW, PieceTextures, Track, centred,
    circle_pose, combo_colour, push_approach, tint,
};
use crate::playfield::PlayfieldLayout;
use crate::slider_body::{
    LegacyBodyColours, Snaking, SnakingTiming, body_mesh, range_distances, snaking_range,
};
use slam_osu::mods::HIDDEN_FADE_OUT_MULTIPLIER;

/// The most sliders a [`LiveSliders`] snapshot holds and a frame draws; later ones are left out.
pub const MAX_LIVE_SLIDERS: usize = 128;

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
                head: None,
                tail: None,
                judgement: None,
            }; MAX_LIVE_SLIDERS],
            len: 0,
            dropped: 0,
        }
    }

    /// Replaces the contents with the sliders alive at the last update of `rules`.
    pub fn capture(&mut self, rules: &OsuRules) {
        self.len = 0;
        self.dropped = 0;
        for slider in rules.live_sliders() {
            if self.len < MAX_LIVE_SLIDERS {
                self.sliders[self.len] = slider;
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
    /// `PlaySliderBody.PathRadius`: `OBJECT_RADIUS * Scale`, in osu!pixels.
    pub radius: f32,
    /// Index of the body's gradient in [`SliderVisuals::gradient`].
    pub gradient: usize,
}

/// The static drawing data of every slider of a beatmap, by object index, with the body
/// gradients of its colours.
#[derive(Debug, Clone)]
pub struct SliderVisuals {
    sliders: Vec<Option<SliderVisual>>,
    gradients: Vec<SliderGradient>,
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
                    radius,
                    gradient,
                })
            })
            .collect();
        SliderVisuals { sliders, gradients }
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

// ---- Drawing ----

/// The skin textures of slider pieces for one frame; missing ones are not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SliderTextures {
    /// `sliderstartcircle` and its overlay, or the hit circle's.
    pub head: PieceTextures,
    /// `sliderendcircle` and its overlay, or the hit circle's.
    pub tail: PieceTextures,
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
}

/// Everything the draw side keeps to draw the sliders of a play.
#[derive(Debug, Clone)]
pub struct HitSliders {
    pub visuals: SliderVisuals,
    pub snaking: Snaking,
}

/// The textures of one frame for [`push_objects`].
#[derive(Debug, Clone, Copy)]
pub struct ObjectTextures<'a> {
    pub circles: &'a CircleTextures,
    pub sliders: &'a SliderTextures,
}

/// Writes the sprites of the `circles` and `sliders` alive at game `time` in lazer's
/// drawing order, and records the slider bodies between them in `bodies`. Objects starting
/// earlier are on top; within a slider, the body, the tail, then the head; every approach
/// circle goes above all objects, in start time order, as lazer's playfield proxies them
/// into a layer of their own.
#[allow(clippy::too_many_arguments)]
pub fn push_objects(
    hit_circles: &HitCircles,
    hit_sliders: &HitSliders,
    batch: &mut SpriteBatch,
    bodies: &mut SliderBodies,
    layout: &PlayfieldLayout,
    circles: &[LiveCircle],
    sliders: &[LiveSlider],
    time: f64,
    textures: ObjectTextures<'_>,
) {
    let style = &hit_circles.style;
    let snaking = hit_sliders.snaking;
    let visuals = &hit_sliders.visuals;

    // Later objects first: merge both lists by descending object index.
    let (mut c, mut s) = (circles.len(), sliders.len());
    while c > 0 || s > 0 {
        let take_circle = s == 0 || (c > 0 && circles[c - 1].index > sliders[s - 1].index);
        if take_circle {
            c -= 1;
            let live = &circles[c];
            if let Some(circle) = hit_circles.visuals.get(live.index) {
                let pose = circle_pose(circle, live.judgement, time, style);
                let piece = PieceTextures::from(textures.circles);
                hit_circles.push_piece(batch, layout, circle, &pose, 1.0, piece, textures.circles);
            }
        } else {
            s -= 1;
            let live = &sliders[s];
            if let Some(slider) = visuals.get(live.index) {
                let pose = slider_pose(slider, live, time, style, snaking);
                push_slider(
                    hit_circles,
                    batch,
                    bodies,
                    layout,
                    live,
                    slider,
                    &pose,
                    textures,
                );
            }
        }
    }

    let Some(approach) = &textures.circles.approach else {
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
                let pose = circle_pose(circle, live.judgement, time, style);
                push_approach(batch, layout, circle, &pose, 1.0, approach);
            }
        } else {
            let live = &sliders[s];
            s += 1;
            if let Some(slider) = visuals.get(live.index) {
                let pose = circle_pose(&slider.head, live.head, time, style);
                let parent = slider_alpha(slider, live, time);
                push_approach(batch, layout, &slider.head, &pose, parent, approach);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_slider(
    hit_circles: &HitCircles,
    batch: &mut SpriteBatch,
    bodies: &mut SliderBodies,
    layout: &PlayfieldLayout,
    live: &LiveSlider,
    slider: &SliderVisual,
    pose: &SliderPose,
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
        });
    }

    let tail = &pose.tail;
    let alpha = pose.object_alpha * tail.object_alpha * tail.piece_alpha * tail.sprite_alpha;
    if alpha > 0.0 {
        let centre = layout.to_window(slider.tail.position);
        let scale = slider.head.scale * layout.scale() * tail.sprite_scale;
        let pieces = &textures.sliders.tail;
        if let Some(t) = &pieces.circle {
            let size = Vec2::new(t.size.x * scale, t.size.y * scale);
            let colour = tint(slider.head.colour, tail.brightness, alpha);
            batch.push(&centred(t, centre, size, colour));
        }
        if let Some(t) = &pieces.overlay {
            let size = Vec2::new(t.size.x * scale, t.size.y * scale);
            let colour = tint(slam_render::Color::WHITE, tail.brightness, alpha);
            batch.push(&centred(t, centre, size, colour));
        }
    }

    hit_circles.push_piece(
        batch,
        layout,
        &slider.head,
        &pose.head,
        pose.object_alpha,
        textures.sliders.head,
        textures.circles,
    );
}
