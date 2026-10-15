//! Spinners as lazer draws them with a legacy skin: the old style (a background, a spinning
//! disc and a metre filling with the progress) or the new style (glow, bottom, top and middle
//! layers), each with the approach circle and the "spin" and "clear" texts.
//!
//! As for hit circles ([`crate::circles`]), the update side copies the spinners alive in
//! [`OsuRules`] into a [`LiveSpinners`] snapshot and the draw side evaluates each one's
//! transforms at the frame's game time ([`spinner_pose`]). Spinners are drawn below every other
//! object, as lazer's playfield proxies them into a layer of their own; earlier spinners below
//! later ones.
//!
//! The legacy spinner is laid out in osu!stable's window space: a 640×480 box centred 8
//! osu!pixels above the spinner, which at the spinner's usual position fills the window's
//! height.
//!
//! Not done: the spins per minute counter and the bonus counter (#152), and lazer's own spinner
//! (`DefaultSpinner`) when no skin has a legacy one. The old-style metre blinks with a random
//! number taken from the frame's game time rather than a running generator. "Clear" starts at
//! the completion time, where lazer notices it a frame later. Transforms start from their
//! property's value at their exact start time, and scales are interpolated before the sprite
//! scale is applied, which may differ from lazer in the last bits.

use slam_osu::objects::OsuHitObjectKind;
use slam_osu::rules::{LiveSpinner, OsuRules};
use slam_osu::{Beatmap, Vec2};
use slam_render::atlas::UvRect;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};
use slam_skin::{Rgba, SkinChain, SkinSource};

use crate::circles::{CircleMods, Easing, Kind, SkinSprite, Track, centred, to_linear, to_srgb};
use crate::playfield::PlayfieldLayout;

/// The most spinners a [`LiveSpinners`] snapshot holds and a frame draws; later ones are left
/// out.
pub const MAX_LIVE_SPINNERS: usize = 16;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySpinner.cs (SPRITE_SCALE, SPINNER_TOP_OFFSET, SPINNER_Y_CENTRE)
/// The scale of every legacy spinner sprite.
const SPRITE_SCALE: f32 = 0.625;
/// The top of osu!stable's window in its gamefield space, which is shifted 16 px down.
const SPINNER_TOP_OFFSET: f32 = 45.0 - 16.0;
/// The spinner's centre, from the top of the box.
const SPINNER_Y_CENTRE: f32 = SPINNER_TOP_OFFSET + 219.0;
/// The legacy spinner's box (`Size`) and its offset from the spinner (`Position`).
const BOX_SIZE: Vec2 = Vec2::new(640.0, 480.0);
const BOX_OFFSET_Y: f32 = -8.0;

/// `DrawableSpinner`'s `fade_out_duration`.
const FADE_OUT_DURATION: f64 = 240.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyOldStyleSpinner.cs (final_metre_height, total_bars)
const FINAL_METRE_HEIGHT: f32 = 692.0 * SPRITE_SCALE;
const TOTAL_BARS: i32 = 10;

/// `LegacyNewStyleSpinner`'s `glowColour`.
const GLOW_COLOUR: [u8; 3] = [3, 151, 255];

/// How long "spin" fades out after a tick is hit (`LegacySpinner.UpdateStateTransforms`).
const TICK_SPIN_FADE: f64 = 300.0;

// ---- Snapshot ----

/// How the hits of a spinner's ticks faded "spin" out: each hit fades it from its current
/// alpha to 0 over 300 ms, the first replacing the fade-out before the end time. The fades
/// chain linearly, so the alpha at the last hit is the alpha at the first times `factor`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickFade {
    /// When the first tick was hit.
    pub first: f64,
    /// When the last tick was hit.
    pub last: f64,
    /// The alpha at the last hit over the alpha at the first.
    pub factor: f32,
}

impl TickFade {
    /// The fade of ticks hit at `hits`, in order; `None` before any hit.
    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/TargetGroupingTransformTracker.cs (AddTransform)
    pub fn new(hits: &[f64]) -> Option<TickFade> {
        let (&first, rest) = hits.split_first()?;
        let mut factor = 1.0;
        let mut last = first;
        for &hit in rest {
            factor *= fade_left(hit - last);
            last = hit;
        }
        Some(TickFade {
            first,
            last,
            factor: factor as f32,
        })
    }
}

/// What is left of a linear fade to 0 over [`TICK_SPIN_FADE`] after `elapsed`.
fn fade_left(elapsed: f64) -> f64 {
    if elapsed >= TICK_SPIN_FADE {
        0.0
    } else {
        1.0 - elapsed / TICK_SPIN_FADE
    }
}

/// A spinner of a [`LiveSpinners`] snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpinnerEntry {
    pub live: LiveSpinner,
    /// How its tick hits faded "spin" out, if any tick was hit.
    pub tick_fade: Option<TickFade>,
}

/// The spinners alive at an update, copied for the draw side without allocations.
#[derive(Debug, Clone)]
pub struct LiveSpinners {
    spinners: [SpinnerEntry; MAX_LIVE_SPINNERS],
    len: usize,
    /// Spinners left out because the snapshot was full.
    dropped: usize,
}

impl Default for LiveSpinners {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveSpinners {
    /// An empty snapshot.
    pub const fn new() -> Self {
        LiveSpinners {
            spinners: [SpinnerEntry {
                live: LiveSpinner {
                    index: 0,
                    is_spinning: false,
                    progress_unclamped: 0.0,
                    bonus_max_spins: 0,
                    progress: 0.0,
                    rotation: 0.0,
                    spun_rotation: 0.0,
                    time_completed: None,
                    last_bonus_hit: None,
                    judgement: None,
                },
                tick_fade: None,
            }; MAX_LIVE_SPINNERS],
            len: 0,
            dropped: 0,
        }
    }

    /// Replaces the contents with the spinners alive at the last update of `rules`.
    pub fn capture(&mut self, rules: &OsuRules) {
        self.clear();
        for live in rules.live_spinners() {
            self.push(SpinnerEntry {
                live,
                tick_fade: TickFade::new(rules.spinner_tick_hits(live.index)),
            });
        }
    }

    /// Appends `spinner`; counted as dropped if full.
    pub fn push(&mut self, spinner: SpinnerEntry) {
        if self.len == MAX_LIVE_SPINNERS {
            self.dropped += 1;
            return;
        }
        self.spinners[self.len] = spinner;
        self.len += 1;
    }

    /// Empties the snapshot.
    pub fn clear(&mut self) {
        self.len = 0;
        self.dropped = 0;
    }

    /// The spinners, in start time order.
    pub fn as_slice(&self) -> &[SpinnerEntry] {
        &self.spinners[..self.len]
    }

    /// How many alive spinners did not fit at the last capture.
    pub fn dropped(&self) -> usize {
        self.dropped
    }
}

// ---- Static data ----

/// One spinner's static drawing data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpinnerVisual {
    pub start_time: f64,
    pub end_time: f64,
    /// Lazer's `Duration`.
    pub duration: f64,
    /// The spinner's centre, in osu!pixels.
    pub position: Vec2,
    pub time_preempt: f64,
    pub time_fade_in: f64,
}

/// The static drawing data of every spinner of a beatmap, by object index.
#[derive(Debug, Clone)]
pub struct SpinnerVisuals {
    spinners: Vec<Option<SpinnerVisual>>,
}

impl SpinnerVisuals {
    /// The spinners of `beatmap` (with its mods applied).
    pub fn new(beatmap: &Beatmap) -> Self {
        let spinners = beatmap
            .hit_objects
            .iter()
            .map(|o| match &o.kind {
                OsuHitObjectKind::Spinner(s) => Some(SpinnerVisual {
                    start_time: o.start_time,
                    end_time: s.end_time,
                    duration: s.duration,
                    position: o.stacked_position(),
                    time_preempt: o.defaults.time_preempt,
                    time_fade_in: o.defaults.time_fade_in,
                }),
                _ => None,
            })
            .collect();
        SpinnerVisuals { spinners }
    }

    /// The spinner with object index `index`; `None` for other objects.
    pub fn get(&self, index: u32) -> Option<&SpinnerVisual> {
        self.spinners.get(index as usize)?.as_ref()
    }
}

/// Which legacy spinner a skin has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerKind {
    /// `LegacyOldStyleSpinner`: `spinner-background`, `spinner-circle`, `spinner-metre`.
    OldStyle,
    /// `LegacyNewStyleSpinner`: `spinner-glow`, `-bottom`, `-top`, `-middle2`, `-middle`.
    NewStyle,
}

/// How every spinner of a play is drawn, apart from the textures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpinnerStyle {
    /// `None` when no skin has a legacy spinner: none is drawn.
    pub kind: Option<SpinnerKind>,
    /// Whether the approach circle is drawn: always for the old style; for the new style only
    /// when `spinner-top` comes from the user's skin.
    pub approach: bool,
    /// Whether the old-style metre blinks (`SpinnerNoBlink` is not set).
    pub blink: bool,
    /// The old-style background's colour (`SpinnerBackground`).
    pub background: Color,
    /// How fast the new-style top turns relative to the rotation: half when `spinner-middle2`
    /// turns with it.
    pub turn_ratio: f32,
    pub mods: CircleMods,
}

impl SpinnerStyle {
    /// The style for a play with `mods`, drawn with `skins`.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs (SpinnerBody)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyNewStyleSpinner.cs (load, Update)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyOldStyleSpinner.cs (load)
    pub fn new(mods: CircleMods, skins: &SkinChain) -> Self {
        // Each skin's transformer is asked in turn, and looks only at its own textures.
        let kind = skins.sources().find_map(|(_, skin)| {
            if skin.texture("spinner-background").is_some() {
                Some(SpinnerKind::OldStyle)
            } else if skin.texture("spinner-top").is_some() {
                Some(SpinnerKind::NewStyle)
            } else {
                None
            }
        });
        let approach = match kind {
            Some(SpinnerKind::OldStyle) => true,
            Some(SpinnerKind::NewStyle) => {
                skins.find_provider(|s| s.texture("spinner-top").is_some())
                    == Some(SkinSource::User)
            }
            None => false,
        };
        let background = skins
            .config(|s| s.custom_colour("SpinnerBackground"))
            .map_or(Rgba::rgb(100, 100, 100), |f| f.value);
        SpinnerStyle {
            kind,
            approach,
            blink: !skins
                .config(|s| s.setting_bool("SpinnerNoBlink"))
                .is_some_and(|f| f.value),
            background: rgba(background),
            turn_ratio: if skins.texture("spinner-middle2").is_some() {
                0.5
            } else {
                1.0
            },
            mods,
        }
    }
}

fn rgba(colour: Rgba) -> Color {
    Color {
        r: f32::from(colour.r) / 255.0,
        g: f32::from(colour.g) / 255.0,
        b: f32::from(colour.b) / 255.0,
        a: f32::from(colour.a) / 255.0,
    }
}

/// The skin textures of spinners for one frame; missing ones are not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SpinnerTextures {
    pub background: Option<SkinSprite>,
    pub circle: Option<SkinSprite>,
    pub metre: Option<SkinSprite>,
    pub glow: Option<SkinSprite>,
    pub bottom: Option<SkinSprite>,
    pub top: Option<SkinSprite>,
    pub middle2: Option<SkinSprite>,
    pub middle: Option<SkinSprite>,
    pub approach: Option<SkinSprite>,
    pub spin: Option<SkinSprite>,
    pub clear: Option<SkinSprite>,
}

// ---- Poses ----

/// A spinner's appearance at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpinnerPose {
    /// The alpha of the whole drawable (`DrawableSpinner`).
    pub object_alpha: f32,
    /// The alpha of the legacy spinner (`LegacySpinner`): its fade-in.
    pub body_alpha: f32,
    /// The approach circle's alpha within the body: 0 under Hidden.
    pub approach_alpha: f32,
    /// The approach circle's scale, before [`SPRITE_SCALE`].
    pub approach_scale: f32,
    pub spin_alpha: f32,
    pub clear_alpha: f32,
    /// The "clear" text's scale, before [`SPRITE_SCALE`].
    pub clear_scale: f32,
    /// The new-style glow's alpha.
    pub glow_alpha: f32,
    /// How white the new-style glow is after a bonus tick, from 0 (its own colour) to 1.
    pub glow_flash: f32,
    /// The new-style fixed middle's colour, white turning red over the spinner's duration.
    pub middle_colour: Color,
    /// The new-style layers' scale, before [`SPRITE_SCALE`].
    pub scale: f32,
    /// Lazer's `Progress`, 0..=1.
    pub progress: f32,
    /// The drawn rotation in degrees, clockwise.
    pub rotation: f32,
}

/// The appearance of `spinner` at game `time`, in the state of `entry`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Drawables/DrawableHitObject.cs (UpdateState, UpdateInitialTransforms, HitStateUpdateTime)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs (UpdateHitStateTransforms)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySpinner.cs (onCompletedChanged, UpdateStateTransforms)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyOldStyleSpinner.cs (UpdateStateTransforms)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyNewStyleSpinner.cs (UpdateStateTransforms, Update)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModHidden.cs (applyHiddenState, getFadeOutParameters, hideSpinnerApproachCircle)
pub fn spinner_pose(
    spinner: &SpinnerVisual,
    entry: &SpinnerEntry,
    time: f64,
    style: &SpinnerStyle,
) -> SpinnerPose {
    let live = &entry.live;
    let start = spinner.start_time;
    let end = spinner.end_time;
    let preempt = spinner.time_preempt;
    let fade_in = spinner.time_fade_in;
    let appear = start - preempt;
    // An unjudged spinner's hit state time is its end time.
    let hit_time = live.judgement.map_or(end, |j| j.time);
    let hit = live.judgement.is_some_and(|j| j.result.is_hit());

    // Set in every state, idle included.
    let mut object = Track::new(Kind::Alpha);
    object.fade_out(hit_time, FADE_OUT_DURATION);

    // `LegacySpinner` fades in on its own.
    let mut body = Track::new(Kind::Alpha);
    body.fade_out(appear, 0.0);
    body.add(start - fade_in, fade_in, 1.0, Easing::None);

    let mut approach_scale = Track::new(Kind::Scale);
    approach_scale.add(start, spinner.duration, 0.1, Easing::None);
    let mut approach_alpha = 1.0;

    // Custom state: Hidden. Spinners are never the first object it spares.
    if let Some(only_approach) = style.mods.hidden {
        if time >= appear {
            approach_alpha = 0.0;
        }
        if !only_approach {
            // From the completed fade-in to the end, then the usual fade-out.
            let fade_start = appear + fade_in;
            let long_fade = end - fade_start;
            object.fade_out(
                fade_start + long_fade,
                preempt * slam_osu::mods::HIDDEN_FADE_OUT_MULTIPLIER,
            );
        }
    }

    // The new-style glow follows the progress every frame; once hit it fades out from there.
    let progress = live.progress;
    let glow_alpha = if hit && time >= hit_time {
        (f64::from(progress) * fade_left_over(time - hit_time, 300.0)) as f32
    } else {
        progress
    };
    // A judgement rewinds the transforms of the legacy spinner's sprites before clearing them:
    // the glow is back to its own colour.
    let glow_flash = match live.last_bonus_hit {
        Some(_) if live.judgement.is_some() && time >= hit_time => 0.0,
        None => 0.0,
        // `FlashColour`: white, then back to the glow's colour over 200 ms.
        Some(hit) => (1.0 - ((time - hit) / 200.0).clamp(0.0, 1.0)) as f32,
    };

    // `fixedMiddle`: white from the preempt, turning red over the duration from the start.
    let red = if time < start {
        0.0
    } else if spinner.duration <= 0.0 {
        1.0
    } else if time == start {
        0.0
    } else {
        ((time - start) / spinner.duration).clamp(0.0, 1.0) as f32
    };
    let fade = to_srgb(1.0 - red);
    let middle_colour = Color::rgb(1.0, fade, fade);

    // Ported from osu-framework 2026.921.1: osu.Framework/Utils/Interpolation.cs (ApplyEasing)
    let scale = 0.8 + Easing::Out.apply(f64::from(progress)) as f32 * 0.2;

    let (clear_alpha, clear_scale) = clear_pose(live, time, end);

    SpinnerPose {
        object_alpha: object.value(1.0, time),
        // `FadeOut` at the preempt: transparent before.
        body_alpha: body.value(0.0, time),
        approach_alpha,
        approach_scale: approach_scale.value(1.86, time),
        spin_alpha: spin_alpha(spinner, entry, time, hit_time),
        clear_alpha,
        clear_scale,
        glow_alpha,
        glow_flash,
        middle_colour,
        scale,
        progress,
        rotation: rotation(live, time),
    }
}

/// The drawn rotation at `time`. The rules stop updating a spinner once judged, but lazer's
/// rotation tracker keeps damping the drawn rotation towards the spun one while it fades out.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Default/SpinnerRotationTracker.cs (Update)
// Ported from osu-framework 2026.921.1: osu.Framework/Utils/Interpolation.cs (Damp)
fn rotation(live: &LiveSpinner, time: f64) -> f32 {
    match live.judgement {
        Some(j) if time > j.time => {
            let left = 0.99f64.powf(time - j.time);
            let target = f64::from(live.spun_rotation);
            (target + (f64::from(live.rotation) - target) * left) as f32
        }
        _ => live.rotation,
    }
}

/// What is left of a linear fade to 0 over `duration` after `elapsed`.
fn fade_left_over(elapsed: f64, duration: f64) -> f64 {
    if elapsed >= duration {
        0.0
    } else {
        1.0 - elapsed / duration
    }
}

/// The alpha of "spin": it fades in before the start and out before the hit state time; while
/// unjudged, each tick hit fades it out instead. A judgement replaces the tick fades.
fn spin_alpha(spinner: &SpinnerVisual, entry: &SpinnerEntry, time: f64, hit_time: f64) -> f32 {
    let half_fade_in = spinner.time_fade_in / 2.0;
    let fade_out = spinner.duration.min(400.0);
    let mut spin = Track::new(Kind::Alpha);
    spin.add(
        spinner.start_time - half_fade_in,
        half_fade_in,
        1.0,
        Easing::None,
    );
    // `FadeOutFromOne`.
    spin.add(hit_time - fade_out, 0.0, 1.0, Easing::None);
    spin.fade_out(hit_time - fade_out, fade_out);

    match entry.tick_fade {
        Some(ticks) if entry.live.judgement.is_none() => {
            let first = spin.value(0.0, ticks.first);
            let last = f64::from(first * ticks.factor);
            (last * fade_left(time.max(ticks.last) - ticks.last)) as f32
        }
        _ => spin.value(0.0, time),
    }
}

/// The alpha and scale of "clear", shown once the spinner is complete until its end time.
/// Its transforms start when the spinner completes, or 400 ms before the end time if that is
/// earlier; a judgement clears them after it has faded out.
fn clear_pose(live: &LiveSpinner, time: f64, end: f64) -> (f32, f32) {
    let Some(completed) = live.time_completed else {
        return (0.0, 1.0);
    };
    if live.judgement.is_some() || time < completed {
        return (0.0, 1.0);
    }
    let start = completed.min(end - 400.0);
    let mut alpha = Track::new(Kind::Alpha);
    alpha.add(start, 400.0, 1.0, Easing::Out);
    alpha.fade_out(end - 50.0, 50.0);
    let mut scale = Track::new(Kind::Scale);
    scale.add(start, 0.0, 2.0, Easing::None);
    scale.add(start, 240.0, 0.8, Easing::Out);
    scale.add(start + 240.0, 160.0, 1.0, Easing::None);
    (alpha.value(0.0, time), scale.value(1.0, time))
}

/// The old-style metre's height at `progress`, blinking the next bar on with the chance of the
/// progress through it, drawn from `random` in 0..1.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyOldStyleSpinner.cs (getMetreHeight)
fn metre_height(progress: f32, blink: bool, random: f64) -> f32 {
    let mut progress = progress * 100.0;
    // The spinner still blinks at full progress.
    if blink {
        progress = progress.min(99.0);
    }
    let mut bars = progress as i32 / 10;
    if blink && random < f64::from((progress as i32 % 10) as f32 / 10.0) {
        bars += 1;
    }
    bars as f32 / TOTAL_BARS as f32 * FINAL_METRE_HEIGHT
}

/// A number in 0..1 drawn from `time` and `index`: the same for the same frame.
fn frame_random(time: f64, index: u32) -> f64 {
    // SplitMix64.
    let mut z = time.to_bits() ^ u64::from(index).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

// ---- Sprites ----

/// Everything the draw side keeps to draw the spinners of a play.
#[derive(Debug, Clone)]
pub struct HitSpinners {
    pub visuals: SpinnerVisuals,
    pub style: SpinnerStyle,
}

impl HitSpinners {
    /// Writes the sprites of the `spinners` alive at game `time`, earlier ones first.
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        spinners: &[SpinnerEntry],
        time: f64,
        textures: &SpinnerTextures,
    ) {
        let Some(kind) = self.style.kind else {
            return;
        };
        for entry in spinners {
            if let Some(spinner) = self.visuals.get(entry.live.index) {
                let pose = spinner_pose(spinner, entry, time, &self.style);
                self.push_spinner(
                    batch,
                    layout,
                    kind,
                    entry.live.index,
                    spinner,
                    &pose,
                    time,
                    textures,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_spinner(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        kind: SpinnerKind,
        index: u32,
        spinner: &SpinnerVisual,
        pose: &SpinnerPose,
        time: f64,
        textures: &SpinnerTextures,
    ) {
        let alpha = pose.object_alpha * pose.body_alpha;
        if alpha <= 0.0 {
            return;
        }
        let window_scale = layout.scale();
        // The top centre of the legacy spinner's box, in osu!pixels.
        let top = Vec2::new(
            spinner.position.x,
            spinner.position.y + BOX_OFFSET_Y - BOX_SIZE.y / 2.0,
        );
        let at = |y: f32| layout.to_window(Vec2::new(top.x, top.y + y));
        let centre = at(SPINNER_Y_CENTRE);
        let white = |a: f32| Color { a, ..Color::WHITE };
        let sprite = |batch: &mut SpriteBatch,
                      texture: &Option<SkinSprite>,
                      centre: Vec2,
                      scale: f32,
                      rotation: f32,
                      colour: Color,
                      blend: Blend| {
            let Some(t) = texture else { return };
            if colour.a <= 0.0 {
                return;
            }
            let scale = scale * window_scale;
            let mut s = centred(
                t,
                centre,
                Vec2::new(t.size.x * scale, t.size.y * scale),
                colour,
            );
            s.rotation = rotation.to_radians();
            s.blend = blend;
            batch.push(&s);
        };
        let approach_alpha = alpha * pose.approach_alpha;
        let approach = |batch: &mut SpriteBatch| {
            if self.style.approach {
                let scale = SPRITE_SCALE * pose.approach_scale;
                let colour = white(approach_alpha);
                sprite(
                    batch,
                    &textures.approach,
                    centre,
                    scale,
                    0.0,
                    colour,
                    Blend::Alpha,
                );
            }
        };

        match kind {
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyOldStyleSpinner.cs (load, Update)
            SpinnerKind::OldStyle => {
                let background = Color {
                    a: alpha * self.style.background.a,
                    ..self.style.background
                };
                sprite(
                    batch,
                    &textures.background,
                    centre,
                    SPRITE_SCALE,
                    0.0,
                    background,
                    Blend::Alpha,
                );
                sprite(
                    batch,
                    &textures.circle,
                    centre,
                    SPRITE_SCALE,
                    pose.rotation,
                    white(alpha),
                    Blend::Alpha,
                );
                if let Some(t) = &textures.metre {
                    let random = frame_random(time, index);
                    let height = metre_height(pose.progress, self.style.blink, random);
                    push_metre(
                        batch,
                        layout,
                        t,
                        top,
                        FINAL_METRE_HEIGHT - height,
                        white(alpha),
                    );
                }
                approach(batch);
            }
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyNewStyleSpinner.cs (load, Update)
            SpinnerKind::NewStyle => {
                let scale = SPRITE_SCALE * pose.scale;
                let top_rotation = pose.rotation * self.style.turn_ratio;
                let glow = glow_colour(pose.glow_flash, alpha * pose.glow_alpha);
                sprite(
                    batch,
                    &textures.glow,
                    centre,
                    scale,
                    0.0,
                    glow,
                    Blend::Additive,
                );
                sprite(
                    batch,
                    &textures.bottom,
                    centre,
                    scale,
                    top_rotation / 3.0,
                    white(alpha),
                    Blend::Alpha,
                );
                sprite(
                    batch,
                    &textures.top,
                    centre,
                    scale,
                    top_rotation,
                    white(alpha),
                    Blend::Alpha,
                );
                sprite(
                    batch,
                    &textures.middle2,
                    centre,
                    scale,
                    pose.rotation,
                    white(alpha),
                    Blend::Alpha,
                );
                let middle = Color {
                    a: alpha,
                    ..pose.middle_colour
                };
                sprite(
                    batch,
                    &textures.middle,
                    centre,
                    scale,
                    0.0,
                    middle,
                    Blend::Alpha,
                );
                approach(batch);
            }
        }

        // `LegacySpinner`'s own sprites, above the rest.
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySpinner.cs (load)
        let spin = white(alpha * pose.spin_alpha);
        sprite(
            batch,
            &textures.spin,
            at(SPINNER_TOP_OFFSET + 335.0),
            SPRITE_SCALE,
            0.0,
            spin,
            Blend::Alpha,
        );
        let clear = white(alpha * pose.clear_alpha);
        let clear_scale = SPRITE_SCALE * pose.clear_scale;
        sprite(
            batch,
            &textures.clear,
            at(SPINNER_TOP_OFFSET + 115.0),
            clear_scale,
            0.0,
            clear,
            Blend::Alpha,
        );
    }
}

/// The new-style glow's colour, `flash` of the way to white (interpolated in linear space,
/// as `FadeColour`), with `alpha`.
fn glow_colour(flash: f32, alpha: f32) -> Color {
    let channel = |c: u8| {
        let from = to_linear(f32::from(c) / 255.0);
        to_srgb(from + flash * (1.0 - from))
    };
    Color {
        r: channel(GLOW_COLOUR[0]),
        g: channel(GLOW_COLOUR[1]),
        b: channel(GLOW_COLOUR[2]),
        a: alpha,
    }
}

/// Writes the old-style metre: its sprite anchored at the box's top left below
/// [`SPINNER_TOP_OFFSET`], masked to below `hidden` osu!pixels from its top.
fn push_metre(
    batch: &mut SpriteBatch,
    layout: &PlayfieldLayout,
    texture: &SkinSprite,
    top: Vec2,
    hidden: f32,
    colour: Color,
) {
    let height = texture.size.y * SPRITE_SCALE;
    if hidden >= height || colour.a <= 0.0 {
        return;
    }
    let hidden = hidden.max(0.0);
    let left = top.x - BOX_SIZE.x / 2.0;
    let corner = layout.to_window(Vec2::new(left, top.y + SPINNER_TOP_OFFSET + hidden));
    let scale = layout.scale();
    let size = [
        texture.size.x * SPRITE_SCALE * scale,
        (height - hidden) * scale,
    ];
    let uv = texture.uv;
    let v0 = uv.v0 + (uv.v1 - uv.v0) * (hidden / height);
    let mut sprite = Sprite::new(
        texture.texture,
        UvRect { v0, ..uv },
        [corner.x, corner.y],
        size,
    );
    sprite.colour = colour;
    batch.push(&sprite);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_fades_chain() {
        assert_eq!(TickFade::new(&[]), None);
        let one = TickFade::new(&[1000.0]).unwrap();
        assert_eq!((one.first, one.last, one.factor), (1000.0, 1000.0, 1.0));
        // 150 ms after the first hit, half is left; the second hit fades out from there.
        let two = TickFade::new(&[1000.0, 1150.0, 1150.0, 1225.0]).unwrap();
        assert_eq!((two.first, two.last), (1000.0, 1225.0));
        assert_eq!(two.factor, 0.5 * 0.75);
        // Once a fade has finished, nothing is left.
        assert_eq!(
            TickFade::new(&[1000.0, 1300.0, 1400.0]).unwrap().factor,
            0.0
        );
    }

    #[test]
    fn metre_blinks_the_next_bar() {
        let bar = FINAL_METRE_HEIGHT / 10.0;
        assert_eq!(metre_height(0.0, true, 0.0), 0.0);
        // 34%: three bars, the fourth with a chance of 0.4.
        assert_eq!(metre_height(0.34, true, 0.39), 4.0 * bar);
        assert_eq!(metre_height(0.34, true, 0.41), 3.0 * bar);
        assert_eq!(metre_height(0.34, false, 0.0), 3.0 * bar);
        // Full: still blinking the last bar unless blinking is off.
        assert_eq!(metre_height(1.0, true, 0.95), 9.0 * bar);
        assert_eq!(metre_height(1.0, true, 0.85), 10.0 * bar);
        assert_eq!(metre_height(1.0, false, 0.0), 10.0 * bar);
    }

    #[test]
    fn frame_random_is_in_range_and_repeatable() {
        for i in 0..100 {
            let r = frame_random(1000.0 + f64::from(i) * 0.37, 3);
            assert!((0.0..1.0).contains(&r));
            assert_eq!(r, frame_random(1000.0 + f64::from(i) * 0.37, 3));
        }
        assert_ne!(frame_random(1000.0, 3), frame_random(1000.0, 4));
    }
}
