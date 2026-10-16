//! Judgement popups and hit lighting as lazer draws them with a legacy skin (ADR-0043).
//!
//! Update side: [`PopupTargets`] decides which judgements show a popup and where;
//! [`JudgementRing`] carries them in the snapshot. Draw side: [`JudgementView`] keeps the alive
//! popups and evaluates their transforms at the frame's game time.

use slam_osu::judgement::{HitResult, Judgement, ObjectRef};
use slam_osu::objects::OsuHitObjectKind;
use slam_osu::rules::RulesOptions;
use slam_osu::{Beatmap, ModSet, Vec2};

use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};
use slam_skin::{AnimationOptions, SIXTY_FRAME_TIME, Skin, SkinChain, SkinVersion, TextureFile};

use crate::circles::{
    CircleSettings, Easing, Kind, SkinSprite, Track, centred, combo_colour, tint,
};
use crate::playfield::PlayfieldLayout;
use crate::ring::EventRing;
use crate::sliders::animation_frame;

/// Judgements a snapshot keeps for the draw side between two frames.
pub const JUDGEMENT_RING_CAPACITY: usize = 64;

/// A judgement that shows a popup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShownJudgement {
    pub object: ObjectRef,
    /// Gameplay time of the judgement (`Result.TimeAbsolute`).
    pub time: f64,
    pub result: HitResult,
    /// Where the popup shows, in osu!pixels.
    pub position: Vec2,
    /// The judged object's scale (`OsuHitObject.Scale`).
    pub scale: f32,
}

impl ShownJudgement {
    const EMPTY: ShownJudgement = ShownJudgement {
        object: ObjectRef {
            index: 0,
            nested: None,
        },
        time: 0.0,
        result: HitResult::None,
        position: Vec2::ZERO,
        scale: 1.0,
    };
}

impl Default for ShownJudgement {
    fn default() -> Self {
        ShownJudgement::EMPTY
    }
}

/// The judgements that show popups, newest last.
pub type JudgementRing = EventRing<ShownJudgement, JUDGEMENT_RING_CAPACITY>;

/// Where an object's popups show and whether it shows them at all.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Target {
    /// Popup position of the object itself.
    position: Vec2,
    scale: f32,
    /// Lazer's `DisplayResult` of the object itself.
    display: bool,
    /// Whether its nested objects show results: slider parts do, spinner ticks do not.
    display_nested: bool,
    /// The object's run in [`PopupTargets::nested`].
    nested: (u32, u32),
}

/// Which judgements of a play show popups, and where. Built at load.
#[derive(Debug, Clone)]
pub struct PopupTargets {
    objects: Vec<Target>,
    /// Popup positions of slider parts, by object then nested index.
    nested: Vec<Vec2>,
}

impl PopupTargets {
    /// The targets of `beatmap` with `mods` applied.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableOsuJudgement.cs (Apply)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (DisplayResult)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinnerTick.cs (DisplayResult)
    pub fn new(beatmap: &Beatmap, mods: &ModSet) -> Self {
        let classic = RulesOptions::for_mods(mods).classic_slider_behaviour;
        let mut nested = Vec::new();
        let objects = beatmap
            .hit_objects
            .iter()
            .map(|o| {
                let start = nested.len() as u32;
                let scale = o.defaults.scale;
                let (position, display, display_nested) = match &o.kind {
                    OsuHitObjectKind::Circle => (o.stacked_position(), true, false),
                    OsuHitObjectKind::Slider(slider) => {
                        nested.extend(slider.nested.iter().map(|n| n.stacked_position()));
                        // The tail sits where the last span ends (`DrawableSliderTail.OnApply`),
                        // the slider's end position.
                        let tail = o.stacked_end_position();
                        // A slider's own result shows at its tail.
                        (tail, classic, true)
                    }
                    OsuHitObjectKind::Spinner(_) => (o.stacked_position(), true, false),
                };
                Target {
                    position,
                    scale,
                    display,
                    display_nested,
                    nested: (start, nested.len() as u32 - start),
                }
            })
            .collect();
        PopupTargets { objects, nested }
    }

    /// The popup of `judgement`, if it shows one.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuPlayfield.cs (onNewResult, judgementPooler)
    pub fn shown(&self, judgement: &Judgement) -> Option<ShownJudgement> {
        if !pooled(judgement.result) {
            return None;
        }
        let target = self.objects.get(judgement.object.index as usize)?;
        let position = match judgement.object.nested {
            None if target.display => target.position,
            Some(n) if target.display_nested && n < target.nested.1 => {
                self.nested[(target.nested.0 + n) as usize]
            }
            _ => return None,
        };
        Some(ShownJudgement {
            object: judgement.object,
            time: judgement.time,
            result: judgement.result,
            position,
            scale: target.scale,
        })
    }
}

/// Whether lazer's osu! playfield pools popups of `result`; others show none.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuPlayfield.cs (JudgementPooler)
fn pooled(result: HitResult) -> bool {
    matches!(
        result,
        HitResult::Great
            | HitResult::Ok
            | HitResult::Meh
            | HitResult::Miss
            | HitResult::LargeTickHit
            | HitResult::SliderTailHit
            | HitResult::LargeTickMiss
            | HitResult::IgnoreMiss
    )
}

// ---- Draw side: poses ----

/// How a result's popup body is drawn, from the skin that provides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PieceKind {
    /// No skin draws it.
    #[default]
    None,
    /// `LegacyJudgementPieceOld`.
    Old,
    /// `LegacyJudgementPieceNew`: the skin has a particle texture for the result.
    New,
    /// `LegacyJudgementPieceSliderTickHit`: `sliderpoint30`/`sliderpoint10` of a skin
    /// older than version 2.
    TickHit,
}

/// How popups of a play are drawn, apart from the textures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JudgementStyle {
    /// Misses fall down when the skin's version is above 1.0.
    pub miss_falls: bool,
    /// Lazer's `HitLighting` setting (on by default).
    pub hit_lighting: bool,
}

/// One body of a popup at one moment: alpha, scale, rotation in degrees and offset in
/// osu!pixels (scaled by the object's scale), and the animation's playback time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PiecePose {
    pub alpha: f32,
    pub scale: f32,
    pub rotation: f32,
    pub offset: Vec2,
}

/// A popup at one moment, relative to its centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopupPose {
    /// The hit lighting's alpha and scale; zero alpha when there is none.
    pub lighting: (f32, f32),
    /// The body: below hit objects for [`PieceKind::New`], above for the others.
    pub main: PiecePose,
    /// The additive copy of the old style a new-style hit shows above hit objects.
    pub temporary: Option<PiecePose>,
    /// The particles' alpha (the piece's and the explosion's) and their time since they
    /// started, in milliseconds.
    pub particles: (f32, f64),
}

/// Popups expire this long after their judgement: the hit lighting ends at 1400 ms, the
/// bodies at 1100 ms (the particles fade with them; their own fade would end at 1500 ms).
pub const POPUP_LIFETIME: f64 = 1400.0;

/// `LegacyJudgementPieceOld`'s and `New`'s `fade_in_length`, `fade_out_delay`, `fade_out_length`.
const FADE_IN: f64 = 120.0;
const FADE_OUT_DELAY: f64 = 500.0;
const FADE_OUT: f64 = 600.0;

/// Particles of a new-style popup (`ParticleExplosion`): count, duration and area.
pub const PARTICLE_COUNT: usize = 150;
const PARTICLE_DURATION: f64 = 1600.0;
const PARTICLE_AREA: f32 = 140.0;

/// The transforms of one legacy piece, relative to its judgement's time.
#[derive(Debug, Clone, Copy)]
struct PieceTracks {
    alpha: Track,
    scale: Track,
    rotation: Track,
    /// The vertical offset; legacy pieces only move vertically.
    y: Track,
}

impl PieceTracks {
    fn new() -> Self {
        PieceTracks {
            alpha: Track::new(Kind::Alpha),
            scale: Track::new(Kind::Scale),
            rotation: Track::new(Kind::Alpha),
            y: Track::new(Kind::Scale),
        }
    }

    /// The pose at `t` milliseconds after the judgement, from an alpha and scale of 1.
    fn at(&self, t: f64) -> PiecePose {
        PiecePose {
            alpha: self.alpha.value(1.0, t),
            scale: self.scale.value(1.0, t),
            rotation: self.rotation.value(0.0, t),
            offset: Vec2::new(0.0, self.y.value(0.0, t)),
        }
    }
}

/// `FadeInFromZero(FADE_IN)` then `Delay(FADE_OUT_DELAY).FadeOut(FADE_OUT)`.
fn fade_in_and_out(alpha: &mut Track) {
    alpha.add(0.0, 0.0, 0.0, Easing::None);
    alpha.add(0.0, FADE_IN, 1.0, Easing::None);
    alpha.add(FADE_OUT_DELAY, FADE_OUT, 0.0, Easing::None);
}

/// The transforms `LegacyJudgementPieceOld.PlayAnimation` adds for `result`; `animated` when
/// its texture has more than one frame, `rotation` the random angle of a miss.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyJudgementPieceOld.cs (PlayAnimation)
fn old_piece(
    result: HitResult,
    final_scale: f32,
    force_transforms: bool,
    animated: bool,
    miss_falls: bool,
    rotation: f32,
) -> PieceTracks {
    let mut p = PieceTracks::new();
    fade_in_and_out(&mut p.alpha);
    // Animated judgements play no transforms, unless they are a new piece's temporary one.
    if animated && !force_transforms {
        return p;
    }
    if result.is_miss() {
        if result != HitResult::Miss {
            // Missed ticks and slider ends.
            p.scale.add(0.0, 0.0, 1.2, Easing::None);
            p.scale.add(0.0, 100.0, 1.0, Easing::In);
            p.alpha
                .add(FADE_OUT_DELAY / 2.0, FADE_OUT, 0.0, Easing::None);
        } else {
            p.scale.add(0.0, 0.0, 1.6, Easing::None);
            p.scale.add(0.0, 100.0, 1.0, Easing::In);
            if miss_falls {
                // `MoveToOffset` reads its start when it starts: after `MoveTo(0, -5)`.
                p.y.add(0.0, 0.0, -5.0, Easing::None);
                p.y.add(0.0, FADE_OUT_DELAY + FADE_OUT, 75.0, Easing::In);
            }
            p.rotation.add(0.0, 0.0, 0.0, Easing::None);
            p.rotation.add(0.0, FADE_IN, rotation, Easing::None);
            p.rotation.add(
                FADE_IN,
                FADE_OUT_DELAY + FADE_OUT - FADE_IN,
                rotation * 2.0,
                Easing::In,
            );
        }
    } else {
        // Stable's scale 0.9 → 1 over 1.0 to 1.4 of the fade-in, forced to 0.95 at 1.2. The
        // `float` factors widen to `double` as in lazer.
        let (four_fifths, fifth) = (FADE_IN * f64::from(0.8f32), FADE_IN * f64::from(0.2f32));
        p.scale.add(0.0, 0.0, 0.6, Easing::None);
        p.scale.add(0.0, four_fifths, 1.1, Easing::None);
        let at_1_0 = four_fifths + fifth;
        p.scale.add(at_1_0, fifth, 0.9, Easing::None);
        let at_1_2 = at_1_0 + fifth;
        p.scale.add(at_1_2, 0.0, 0.95, Easing::None);
        p.scale.add(at_1_2, fifth, final_scale, Easing::None);
    }
    p
}

/// Lazer's `RNG.NextSingle(min, max)` from a deterministic source (ADR-0043).
// Ported from osu-framework 2026.921.1: osu.Framework/Utils/RNG.cs (NextSingle)
fn next_single(rng: &mut VisualRng, min: f32, max: f32) -> f32 {
    min + rng.next_double() as f32 * (max - min)
}

/// A pseudo-random sequence from a seed (SplitMix64): the randomness of visual effects,
/// the same for the same judgement on every frame and every run (ADR-0043).
#[derive(Debug, Clone, Copy)]
pub(crate) struct VisualRng(u64);

impl VisualRng {
    /// The sequence of `judgement`'s popup, and of its particle `stream` (0 for the popup
    /// itself).
    pub(crate) fn new(judgement: &ShownJudgement, stream: u64) -> Self {
        let object = u64::from(judgement.object.index) << 32
            | u64::from(judgement.object.nested.unwrap_or(u32::MAX));
        let mut rng = VisualRng(object ^ judgement.time.to_bits().rotate_left(17));
        rng.0 ^=
            (judgement.result.index() as u64) << 56 ^ stream.wrapping_mul(0xA24B_AED4_963E_E407);
        rng.next_u64();
        rng
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1), as `Random.NextDouble`.
    pub(crate) fn next_double(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

/// The pose of a popup of `judgement` drawn as `kind` at game `time`; `animated` when its
/// texture has more than one frame.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Judgements/DrawableJudgement.cs (runAnimation)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableOsuJudgement.cs (ApplyHitAnimations)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyJudgementPieceNew.cs (PlayAnimation)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyJudgementPieceSliderTickHit.cs (PlayAnimation)
pub fn popup_pose(
    judgement: &ShownJudgement,
    kind: PieceKind,
    animated: bool,
    style: &JudgementStyle,
    time: f64,
) -> PopupPose {
    let t = time - judgement.time;
    let result = judgement.result;
    let rotation = if result == HitResult::Miss {
        next_single(&mut VisualRng::new(judgement, 0), -8.6, 8.6)
    } else {
        0.0
    };

    // The hit lighting of hits, transparent for ticks (`SkinnableLighting.updateColour`).
    let lighting = if style.hit_lighting && result.is_hit() && !result.is_tick() {
        let mut alpha = Track::new(Kind::Alpha);
        alpha.add(0.0, 200.0, 1.0, Easing::None);
        alpha.add(400.0, 1000.0, 0.0, Easing::None);
        let mut scale = Track::new(Kind::Scale);
        scale.add(0.0, 0.0, 0.8, Easing::None);
        scale.add(0.0, 600.0, 1.2, Easing::Out);
        (alpha.value(0.0, t), scale.value(1.0, t))
    } else {
        (0.0, 1.0)
    };

    let hidden = PiecePose {
        alpha: 0.0,
        scale: 1.0,
        rotation: 0.0,
        offset: Vec2::ZERO,
    };
    let mut pose = PopupPose {
        lighting,
        main: hidden,
        temporary: None,
        particles: (0.0, 0.0),
    };
    match kind {
        PieceKind::None => {}
        PieceKind::Old => {
            pose.main = old_piece(result, 1.0, false, animated, style.miss_falls, rotation).at(t);
        }
        PieceKind::TickHit => {
            let mut p = PieceTracks::new();
            p.y.add(0.0, 300.0, -10.0, Easing::Out);
            p.alpha.add(300.0, 60.0, 0.0, Easing::None);
            pose.main = p.at(t);
        }
        PieceKind::New => {
            let mut piece = Track::new(Kind::Alpha);
            fade_in_and_out(&mut piece);
            let piece_alpha = piece.value(1.0, t);
            let mut main_scale = Track::new(Kind::Scale);
            if !animated {
                main_scale.add(0.0, 0.0, 0.9, Easing::None);
                main_scale.add(0.0, FADE_OUT_DELAY + FADE_OUT, 1.05, Easing::None);
            }
            pose.main = PiecePose {
                alpha: piece_alpha,
                scale: main_scale.value(1.0, t),
                rotation: 0.0,
                offset: Vec2::ZERO,
            };
            // The particles start 100 ms into their animation.
            let mut explosion = Track::new(Kind::Alpha);
            explosion.add(-100.0, 0.0, 1.0, Easing::None);
            explosion.add(-100.0, PARTICLE_DURATION, 0.0, Easing::None);
            pose.particles = (piece_alpha * explosion.value(1.0, t), t + 100.0);
            // Misses have no particles and use the old piece, so this is a hit: the old style
            // shows briefly on top, additively.
            let old = old_piece(result, 1.05, true, animated, style.miss_falls, rotation);
            let mut temporary = old.at(t);
            temporary.alpha = temporary_alpha(t) * piece_alpha;
            pose.temporary = Some(temporary);
        }
    }
    pose
}

/// The alpha of a new piece's temporary old style `t` milliseconds after the judgement:
/// `Hide()`, then `Delay(-16).FadeTo(0.5f, 56, Easing.Out).Then().FadeOut(300)`. The fade
/// to 0.5 removes the old piece's own fades (they start after it); it is added once the
/// judgement's time has come, so the hide applies at once and the fade starts from 0.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyJudgementPieceNew.cs (PlayAnimation)
fn temporary_alpha(t: f64) -> f32 {
    let mut alpha = Track::new(Kind::Alpha);
    alpha.add(-16.0, 56.0, 0.5, Easing::Out);
    alpha.add(40.0, 300.0, 0.0, Easing::None);
    alpha.value(0.0, t)
}

/// One particle of a popup at `time` milliseconds since the particles started: its offset
/// from the centre in osu!pixels at scale 1 and its alpha.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/ParticleExplosion.cs (ParticlePart)
pub fn particle(judgement: &ShownJudgement, index: usize, time: f64) -> (Vec2, f32) {
    let mut rng = VisualRng::new(judgement, index as u64 + 1);
    let distance = next_single(&mut rng, 0.0, 0.5);
    let min = PARTICLE_DURATION / 3.0;
    let duration = min + rng.next_double() * (PARTICLE_DURATION - min);
    let direction = next_single(&mut rng, 0.0, std::f32::consts::PI * 2.0);
    let progress = (time / duration).clamp(0.0, 1.0) as f32;
    let travelled = distance * progress;
    let offset = Vec2::new(direction.sin(), direction.cos()) * (travelled * PARTICLE_AREA);
    (offset, 1.0 - progress)
}

// ---- Draw side: textures ----

/// The popup kinds a play draws, by [`slot`].
pub const PIECE_SLOTS: usize = 8;

/// The most animation frames of all popup bodies together; later ones are left out.
pub const MAX_JUDGEMENT_FRAMES: usize = 256;

/// The slot of a pooled result's textures.
pub(crate) fn slot(result: HitResult) -> Option<usize> {
    Some(match result {
        HitResult::Miss => 0,
        HitResult::Meh => 1,
        HitResult::Ok => 2,
        HitResult::Great => 3,
        HitResult::LargeTickMiss => 4,
        HitResult::IgnoreMiss => 5,
        HitResult::LargeTickHit => 6,
        HitResult::SliderTailHit => 7,
        _ => return None,
    })
}

/// The textures of one result's popup.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PieceSprites {
    pub kind: PieceKind,
    /// The body's frames in [`JudgementTextures::frames`]: start and count.
    pub frames: (u16, u16),
    /// The time each frame is shown, in milliseconds.
    pub frame_length: f64,
    /// The particle texture of a new-style piece.
    pub particle: Option<SkinSprite>,
}

/// The textures of judgement popups for one frame; popups whose textures are not all
/// uploaded are not drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JudgementTextures {
    pub pieces: [PieceSprites; PIECE_SLOTS],
    pub frames: [Option<SkinSprite>; MAX_JUDGEMENT_FRAMES],
    /// `lighting`.
    pub lighting: Option<SkinSprite>,
}

impl Default for JudgementTextures {
    fn default() -> Self {
        JudgementTextures {
            pieces: [PieceSprites::default(); PIECE_SLOTS],
            frames: [None; MAX_JUDGEMENT_FRAMES],
            lighting: None,
        }
    }
}

impl JudgementTextures {
    /// The body of `pieces` shown `playback` milliseconds after the judgement: animations
    /// play once from their first frame and stay on the last.
    fn frame(&self, piece: &PieceSprites, playback: f64) -> Option<&SkinSprite> {
        let (start, count) = (usize::from(piece.frames.0), usize::from(piece.frames.1));
        let frame = animation_frame(playback, piece.frame_length, count, false);
        self.frames.get(start + frame)?.as_ref()
    }
}

// ---- Draw side: alive popups ----

/// The most popups alive at once; beyond it the oldest goes.
pub const MAX_POPUPS: usize = 128;

// `push_above` orders popups by `u8` indices.
const _: () = assert!(MAX_POPUPS <= 256);

/// The popups of a play on the draw side.
#[derive(Debug, Clone)]
pub struct JudgementView {
    pub style: JudgementStyle,
    /// The accent (combo) colour of each object, by index, for the hit lighting.
    colours: Vec<Color>,
    /// Alive popups, oldest first.
    popups: [ShownJudgement; MAX_POPUPS],
    len: usize,
    /// Judgements of the ring taken.
    seen: u64,
    restarts: u32,
    /// Popups that went early: the ring overflowed between frames or too many were alive.
    dropped: u64,
}

impl JudgementView {
    /// The popups of `beatmap` (with its mods applied), lit in the objects' combo colours from
    /// `beatmap_skin` and `skins`, as [`crate::circles`] colours circles.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyJudgementPieceOld.cs (PlayAnimation)
    pub fn new(
        beatmap: &Beatmap,
        settings: CircleSettings,
        beatmap_skin: Option<&Skin>,
        skins: &SkinChain,
    ) -> Self {
        let beatmap_colours =
            beatmap_skin.filter(|s| settings.beatmap_colours && !s.ini().combo_colours.is_empty());
        let colours = beatmap
            .hit_objects
            .iter()
            .map(|o| combo_colour(&o.combo, beatmap_colours, skins))
            .collect();
        // The version the skin source gives: the first skin's.
        let version = skins.config(|s| Some(s.version())).map(|f| f.value);
        JudgementView {
            style: JudgementStyle {
                miss_falls: version.is_some_and(|v| v > SkinVersion::V1),
                hit_lighting: true,
            },
            colours,
            popups: [ShownJudgement::EMPTY; MAX_POPUPS],
            len: 0,
            seen: 0,
            restarts: 0,
            dropped: 0,
        }
    }

    /// Takes the new judgements of `ring` and drops the popups expired by game `time`. A new
    /// popup of an object replaces its older ones (`JudgementContainer.Add`); a restart
    /// clears them all.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/JudgementContainer.cs (Add)
    pub fn update(&mut self, ring: &JudgementRing, restarts: u32, time: f64) {
        if restarts != self.restarts {
            self.restarts = restarts;
            self.len = 0;
        }
        self.dropped += ring.skipped(self.seen);
        for shown in ring.since(self.seen) {
            self.retain(|p| p.object != shown.object);
            if self.len == MAX_POPUPS {
                self.retain_from(1);
                self.dropped += 1;
            }
            self.popups[self.len] = *shown;
            self.len += 1;
        }
        self.seen = ring.written();
        self.retain(|p| time < p.time + POPUP_LIFETIME);
    }

    fn retain(&mut self, keep: impl Fn(&ShownJudgement) -> bool) {
        let mut kept = 0;
        for i in 0..self.len {
            if keep(&self.popups[i]) {
                self.popups[kept] = self.popups[i];
                kept += 1;
            }
        }
        self.len = kept;
    }

    fn retain_from(&mut self, first: usize) {
        self.popups.copy_within(first..self.len, 0);
        self.len -= first;
    }

    /// The alive popups, oldest first.
    pub fn popups(&self) -> &[ShownJudgement] {
        &self.popups[..self.len]
    }

    /// Popups that went before their end.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Writes the parts of the popups below hit objects at game `time`: hit lighting, and the
    /// particles and body of new-style popups, in arrival order.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuPlayfield.cs (judgementLayer)
    pub fn push_below(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        time: f64,
        textures: &JudgementTextures,
    ) {
        for popup in self.popups() {
            let Some(piece) = slot(popup.result).map(|s| &textures.pieces[s]) else {
                continue;
            };
            if time < popup.time {
                continue;
            }
            let pose = popup_pose(popup, piece.kind, piece.frames.1 > 1, &self.style, time);
            let centre = layout.to_window(popup.position);
            let scale = popup.scale * layout.scale();
            if let Some(light) = &textures.lighting
                && pose.lighting.0 > 0.0
            {
                let accent = self
                    .colours
                    .get(popup.object.index as usize)
                    .copied()
                    .unwrap_or(Color::WHITE);
                let size = light.size * (scale * pose.lighting.1);
                let colour = tint(accent, 1.0, pose.lighting.0);
                batch.push(&Sprite {
                    blend: Blend::Additive,
                    ..centred(light, centre, size, colour)
                });
            }
            if piece.kind != PieceKind::New {
                continue;
            }
            if let Some(texture) = &piece.particle
                && pose.particles.0 > 0.0
            {
                let size = texture.size * scale;
                for i in 0..PARTICLE_COUNT {
                    let (offset, alpha) = particle(popup, i, pose.particles.1);
                    let alpha = alpha * pose.particles.0;
                    if alpha <= 0.0 {
                        continue;
                    }
                    let colour = tint(Color::WHITE, 1.0, alpha);
                    batch.push(&Sprite {
                        blend: Blend::Additive,
                        ..centred(texture, centre + offset * scale, size, colour)
                    });
                }
            }
            if let Some(body) = textures.frame(piece, time - popup.time) {
                push_body(batch, body, centre, scale, &pose.main, Blend::Alpha);
            }
        }
    }

    /// Writes the parts of the popups above hit objects at game `time`: old-style bodies and
    /// the temporary old style of new ones, later judgements on top.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuPlayfield.cs (judgementAboveHitObjectLayer, onNewResult)
    pub fn push_above(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        time: f64,
        textures: &JudgementTextures,
    ) {
        // By judgement time, then arrival: a stable insertion sort of indices.
        let mut order = [0u8; MAX_POPUPS];
        for i in 0..self.len {
            let mut j = i;
            while j > 0 && self.popups[usize::from(order[j - 1])].time > self.popups[i].time {
                order[j] = order[j - 1];
                j -= 1;
            }
            order[j] = i as u8;
        }
        for &i in &order[..self.len] {
            let popup = &self.popups[usize::from(i)];
            let Some(piece) = slot(popup.result).map(|s| &textures.pieces[s]) else {
                continue;
            };
            if time < popup.time || piece.kind == PieceKind::None {
                continue;
            }
            let pose = popup_pose(popup, piece.kind, piece.frames.1 > 1, &self.style, time);
            let Some(body) = textures.frame(piece, time - popup.time) else {
                continue;
            };
            let centre = layout.to_window(popup.position);
            let scale = popup.scale * layout.scale();
            match (piece.kind, pose.temporary) {
                (PieceKind::New, Some(temporary)) => {
                    push_body(batch, body, centre, scale, &temporary, Blend::Additive);
                }
                (PieceKind::New, None) => {}
                _ => push_body(batch, body, centre, scale, &pose.main, Blend::Alpha),
            }
        }
    }
}

/// Writes a popup body in `pose` around `centre`, `scale` window units per osu!pixel.
fn push_body(
    batch: &mut SpriteBatch,
    body: &SkinSprite,
    centre: Vec2,
    scale: f32,
    pose: &PiecePose,
    blend: Blend,
) {
    if pose.alpha <= 0.0 {
        return;
    }
    let size = body.size * (scale * pose.scale);
    let colour = tint(Color::WHITE, 1.0, pose.alpha);
    batch.push(&Sprite {
        rotation: pose.rotation.to_radians(),
        blend,
        ..centred(body, centre + pose.offset * scale, size, colour)
    });
}

// ---- Skin lookup ----

/// Where a result's popup textures come from, found at load.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PieceSource {
    pub kind: PieceKind,
    pub frames: Vec<TextureFile>,
    pub frame_length: f64,
    pub particle: Option<TextureFile>,
}

/// The results with popup textures, in [`slot`] order.
const SLOT_RESULTS: [HitResult; PIECE_SLOTS] = [
    HitResult::Miss,
    HitResult::Meh,
    HitResult::Ok,
    HitResult::Great,
    HitResult::LargeTickMiss,
    HitResult::IgnoreMiss,
    HitResult::LargeTickHit,
    HitResult::SliderTailHit,
];

/// The popup textures of each result in `skins`, in [`slot`] order: the first skin that draws
/// a result draws it.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkin.cs (GetDrawableComponent, getParticleTexture, getJudgementAnimation)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs (GetDrawableComponent)
pub fn piece_sources(skins: &SkinChain) -> [PieceSource; PIECE_SLOTS] {
    SLOT_RESULTS.map(|result| {
        skins
            .sources()
            .find_map(|(_, skin)| piece_source(skin, result))
            .unwrap_or_default()
    })
}

/// What `skin` draws for `result`; `None` to ask the next skin.
fn piece_source(skin: &Skin, result: HitResult) -> Option<PieceSource> {
    // Skins older than version 2 draw slider ticks and ends with stable's score points.
    let slider_point = |result| {
        if skin.version() >= SkinVersion::new(2, 0) {
            return None;
        }
        skin.texture(if result == HitResult::LargeTickHit {
            "sliderpoint30"
        } else {
            "sliderpoint10"
        })
    };
    match result {
        HitResult::LargeTickHit | HitResult::SliderTailHit => {
            return slider_point(result).map(|texture| PieceSource {
                kind: PieceKind::TickHit,
                frames: vec![texture],
                frame_length: SIXTY_FRAME_TIME,
                particle: None,
            });
        }
        // A skin that shows hit ticks keeps their misses to itself, drawn or not.
        HitResult::LargeTickMiss | HitResult::IgnoreMiss => {
            let hit = if result == HitResult::LargeTickMiss {
                HitResult::LargeTickHit
            } else {
                HitResult::SliderTailHit
            };
            if slider_point(hit).is_some() {
                return Some(legacy_piece(skin, result).unwrap_or_default());
            }
        }
        _ => {}
    }
    legacy_piece(skin, result)
}

/// `LegacySkin`'s own judgement piece for `result`.
fn legacy_piece(skin: &Skin, result: HitResult) -> Option<PieceSource> {
    let name = match result {
        HitResult::Miss => "hit0",
        HitResult::LargeTickMiss => "slidertickmiss",
        HitResult::IgnoreMiss => "sliderendmiss",
        HitResult::Meh => "hit50",
        HitResult::Ok => "hit100",
        HitResult::Great => "hit300",
        _ => return None,
    };
    let options = AnimationOptions {
        animatable: true,
        looping: false,
        apply_config_frame_rate: false,
        separator: "-",
    };
    let animation = skin.animation(name, options)?;
    let particle = match result {
        HitResult::Meh => skin.texture("particle50"),
        HitResult::Ok => skin.texture("particle100"),
        HitResult::Great => skin.texture("particle300"),
        _ => None,
    };
    Some(PieceSource {
        kind: if particle.is_some() {
            PieceKind::New
        } else {
            PieceKind::Old
        },
        frames: animation.frames,
        frame_length: animation.frame_length_ms,
        particle,
    })
}
