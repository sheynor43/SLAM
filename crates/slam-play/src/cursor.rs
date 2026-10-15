//! The gameplay cursor of a legacy skin and its trail (ADR-0042).
//!
//! Update side: [`CursorPath`] keeps the cursor positions gameplay took, [`ExpandTracker`]
//! the moments the cursor expands and contracts; both go into the snapshot. Draw side:
//! [`CursorView`] turns them into the cursor and its trail, with the trail's own clock.

use slam_osu::Vec2;
use slam_osu::replay::{ActionEvent, Actions};
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};
use slam_skin::{SkinChain, SkinSource};

use crate::circles::{Easing, SkinSprite};
use crate::playfield::PlayfieldLayout;

/// Positions a snapshot keeps for the draw side: 8000 Hz of input at 31 frames a second.
pub const CURSOR_PATH_CAPACITY: usize = 256;

/// The cursor positions gameplay took, newest last, in osu!pixels: a ring of the last
/// [`CURSOR_PATH_CAPACITY`] with a count of all ever written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorPath {
    points: [Vec2; CURSOR_PATH_CAPACITY],
    written: u64,
}

impl Default for CursorPath {
    fn default() -> Self {
        CursorPath {
            points: [Vec2::ZERO; CURSOR_PATH_CAPACITY],
            written: 0,
        }
    }
}

impl CursorPath {
    /// Appends a position, overwriting the oldest one when full.
    pub fn push(&mut self, position: Vec2) {
        self.points[(self.written % CURSOR_PATH_CAPACITY as u64) as usize] = position;
        self.written += 1;
    }

    /// Positions ever written; the cursor has not moved while this is 0.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// The positions written after the first `seen`, oldest first. Those overwritten already
    /// are skipped.
    pub fn since(&self, seen: u64) -> impl Iterator<Item = Vec2> + '_ {
        let start = seen.max(self.written.saturating_sub(CURSOR_PATH_CAPACITY as u64));
        (start..self.written).map(|i| self.points[(i % CURSOR_PATH_CAPACITY as u64) as usize])
    }
}

/// Game times of the cursor's last expansion and contraction (`OsuCursor.Expand`, `Contract`);
/// negative infinity when none happened.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpandTimes {
    pub expanded_at: f64,
    pub contracted_at: f64,
}

impl Default for ExpandTimes {
    fn default() -> Self {
        ExpandTimes {
            expanded_at: f64::NEG_INFINITY,
            contracted_at: f64::NEG_INFINITY,
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursor.cs (pressed_scale, Expand, Contract)
const PRESSED_SCALE: f64 = 1.3;
const RELEASED_SCALE: f64 = 1.0;
const EXPAND_DURATION: f64 = 100.0;

impl ExpandTimes {
    /// The legacy cursor's scale at game time `time`: each expansion goes from 1 to 1.3 in
    /// 100 ms, a contraction back to 1 in 100 ms from wherever the expansion was. With
    /// `CursorExpand` off the cursor never expands.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursor.cs (Expand, Contract)
    pub fn scale(&self, time: f64, expand: bool) -> f32 {
        if !expand || self.expanded_at == f64::NEG_INFINITY {
            return RELEASED_SCALE as f32;
        }
        let expanding = |t: f64| {
            let progress = ((t - self.expanded_at) / EXPAND_DURATION).clamp(0.0, 1.0);
            RELEASED_SCALE + (PRESSED_SCALE - RELEASED_SCALE) * Easing::Out.apply(progress)
        };
        if self.contracted_at < self.expanded_at {
            return expanding(time) as f32;
        }
        let from = expanding(self.contracted_at);
        let progress = ((time - self.contracted_at) / EXPAND_DURATION).clamp(0.0, 1.0);
        (from + (RELEASED_SCALE - from) * Easing::Out.apply(progress)) as f32
    }
}

/// Follows the left and right buttons of gameplay as lazer's `OsuCursorContainer` does: every
/// press expands the cursor, releasing the last one held contracts it.
#[derive(Debug, Clone, Default)]
pub struct ExpandTracker {
    down: u32,
    times: ExpandTimes,
}

impl ExpandTracker {
    /// Takes a step's presses and releases.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/OsuCursorContainer.cs (OnPressed, OnReleased)
    pub fn apply(&mut self, events: &[ActionEvent]) {
        for event in events {
            if event.action != Actions::LEFT && event.action != Actions::RIGHT {
                continue;
            }
            if event.pressed {
                self.down += 1;
                self.times.expanded_at = event.time;
                // Expanding starts from 1 whatever came before: a contraction at the same
                // time is over.
                self.times.contracted_at = f64::NEG_INFINITY;
            } else {
                self.down = self.down.saturating_sub(1);
                if self.down == 0 {
                    self.times.contracted_at = event.time;
                }
            }
        }
    }

    pub fn times(&self) -> ExpandTimes {
        self.times
    }

    /// Nothing held, never expanded: a play that starts over has a new cursor.
    pub fn reset(&mut self) {
        *self = ExpandTracker::default();
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursor.cs (REVOLUTION_DURATION)
const REVOLUTION_DURATION: f64 = 10000.0;

/// The legacy cursor's rotation at game time `time`, in degrees clockwise: a turn every 10 s
/// with `CursorRotate`. Lazer counts from when the cursor loads; the phase differs.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursor.cs (LoadComplete)
pub fn rotation(time: f64, rotate: bool) -> f32 {
    if !rotate {
        return 0.0;
    }
    (time.rem_euclid(REVOLUTION_DURATION) / REVOLUTION_DURATION * 360.0) as f32
}

// ---- Skin ----

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkin.cs (STABLE_MAGIC_SCALE_FACTOR)
/// Lazer shrinks the cursor's and the trail's textures by this (`NonPlayfieldSprite`).
const STABLE_MAGIC_SCALE_FACTOR: f32 = 1.6;

/// The skin configuration of the cursor and its trail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorSkinConfig {
    /// `CursorCentre` of the skin with `cursor`: the cursor's origin is its centre, otherwise
    /// its top-left corner.
    pub centre: bool,
    /// `CursorRotate` of the skin with `cursor`.
    pub rotate: bool,
    /// `CursorExpand`, the first skin that sets it.
    pub expand: bool,
    /// `CursorTrailRotate` of the skin with `cursortrail`.
    pub trail_rotate: bool,
    /// `CursorCentre` of the skin with `cursortrail`, for a disjoint trail's origin.
    pub trail_centre: bool,
    /// Whether the trail is disjoint: the skin with `cursor` has no `cursormiddle`.
    pub disjoint: bool,
}

impl Default for CursorSkinConfig {
    fn default() -> Self {
        CursorSkinConfig {
            centre: true,
            rotate: true,
            expand: true,
            trail_rotate: true,
            trail_centre: true,
            disjoint: true,
        }
    }
}

impl CursorSkinConfig {
    /// Lazer's lookups: the cursor and the trail read their skin, `CursorExpand` the chain.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursor.cs (load)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorTrail.cs (load)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/OsuCursor.cs (SkinChanged)
    pub fn new(skins: &SkinChain) -> Self {
        let sources = CursorSources::new(skins);
        let cursor = sources.cursor.map(|s| skins.skin(s));
        let trail = sources.trail.map(|s| skins.skin(s));
        CursorSkinConfig {
            centre: cursor.is_none_or(|s| s.cursor_centre()),
            rotate: cursor.is_none_or(|s| s.cursor_rotate()),
            expand: skins
                .config(|s| s.setting_bool("CursorExpand"))
                .is_none_or(|f| f.value),
            trail_rotate: trail.is_none_or(|s| s.cursor_trail_rotate()),
            trail_centre: trail.is_none_or(|s| s.cursor_centre()),
            // Stable picks the trail's style by the skin the cursor comes from.
            disjoint: cursor.is_none_or(|s| s.texture("cursormiddle").is_none()),
        }
    }
}

/// The skins the cursor and the trail come from: lazer's legacy transformer of the first skin
/// with `cursor` (`cursortrail`) makes the component, which then reads that skin only.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs (GetDrawableComponent)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorSources {
    pub cursor: Option<SkinSource>,
    pub trail: Option<SkinSource>,
}

impl CursorSources {
    pub fn new(skins: &SkinChain) -> Self {
        CursorSources {
            cursor: skins.find_provider(|s| s.texture("cursor").is_some()),
            trail: skins.find_provider(|s| s.texture("cursortrail").is_some()),
        }
    }
}

/// The cursor's textures for one frame; missing ones are not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CursorTextures {
    pub cursor: Option<SkinSprite>,
    pub middle: Option<SkinSprite>,
    pub trail: Option<SkinSprite>,
    pub config: CursorSkinConfig,
}

// ---- Trail ----

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/CursorTrail.cs (max_sprites)
/// Trail parts alive at once.
pub const MAX_TRAIL_PARTS: usize = 2048;

/// Sprites a frame of the cursor writes at most: the trail, `cursor` and `cursormiddle`.
pub const MAX_CURSOR_SPRITES: usize = MAX_TRAIL_PARTS + 2;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorTrail.cs (disjoint_trail_time_separation)
const DISJOINT_TRAIL_TIME_SEPARATION: f64 = 1000.0 / 60.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/CursorTrail.cs (Update)
const FADE_CLOCK_RESET_THRESHOLD: f32 = 1_000_000.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorTrail.cs (FadeExponent)
const FADE_EXPONENT: f32 = 1.0;

/// Lazer's `GameplayCursorSize` until settings exist (#98). It stands for the cursor's
/// `CursorScale` too (size × mod adjustment × automatic size by CS), which differ from it once
/// those exist; `IntervalMultiplier` takes the size alone.
const CURSOR_SIZE: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct TrailPart {
    /// In osu!pixels.
    position: Vec2,
    /// Fade clock value at which the part is gone.
    time: f32,
    scale: f32,
    /// `false` for a part never written (lazer's `InvalidationID == -1`).
    used: bool,
}

const UNUSED_PART: TrailPart = TrailPart {
    position: Vec2::ZERO,
    time: 0.0,
    scale: 1.0,
    used: false,
};

// Ported from osu-framework 2026.921.1: osu.Framework/Input/InputResampler.cs
/// Reduces cursor positions to the corners of their path; raw input (with a fraction) passes
/// as is once seen.
#[derive(Debug, Clone, Copy, Default)]
struct InputResampler {
    last_relevant: Option<Vec2>,
    last_actual: Option<Vec2>,
    is_raw_input: bool,
}

impl InputResampler {
    fn add_position(&mut self, position: Vec2) -> Option<Vec2> {
        if self.is_raw_input {
            self.last_relevant = Some(position);
            self.last_actual = Some(position);
            return Some(position);
        }
        // HD if it has fractions.
        if position.x - position.x.trunc() != 0.0 {
            self.is_raw_input = true;
        }
        let (Some(last_relevant), Some(last_actual)) = (self.last_relevant, self.last_actual)
        else {
            self.last_relevant = Some(position);
            self.last_actual = Some(position);
            return Some(position);
        };
        let diff = position - last_relevant;
        let distance = diff.length();
        let direction = diff / distance;
        let real_diff = position - last_actual;
        let real_movement_distance = real_diff.length();
        if real_movement_distance < 1.0 {
            return None;
        }
        self.last_actual = Some(position);
        // Don't update when it moved less than 10 pixels from the last position in a straight
        // fashion, but never update when it's less than 2 pixels.
        if (distance < 10.0 && direction.dot(real_diff / real_movement_distance) > 0.7)
            || distance < 2.0
        {
            return None;
        }
        self.last_relevant = Some(position);
        Some(position)
    }
}

/// The legacy cursor trail: lazer's `CursorTrail` with `LegacyCursorTrail`'s behaviour.
/// Fades by its own clock, the frames' engine time.
#[derive(Debug, Clone)]
pub struct CursorTrail {
    parts: Vec<TrailPart>,
    current_index: usize,
    /// Engine time of the trail clock's zero, set by the first frame.
    clock_start_ns: Option<u64>,
    /// Trail clock time the fade clock counts from, in milliseconds.
    time_offset: f64,
    /// Fade clock: elapsed time in fade durations.
    time: f32,
    last_position: Option<Vec2>,
    resampler: InputResampler,
    /// The disjoint trail's cursor position and the clock time it last left a part.
    current_position: Option<Vec2>,
    last_trail_time: f64,
}

impl Default for CursorTrail {
    fn default() -> Self {
        CursorTrail::new()
    }
}

impl CursorTrail {
    /// An empty trail. Allocates its parts once.
    pub fn new() -> Self {
        CursorTrail {
            parts: vec![UNUSED_PART; MAX_TRAIL_PARTS],
            current_index: 0,
            clock_start_ns: None,
            time_offset: 0.0,
            time: 0.0,
            last_position: None,
            resampler: InputResampler::default(),
            current_position: None,
            last_trail_time: 0.0,
        }
    }

    /// Forgets every part, as a new trail.
    pub fn reset(&mut self) {
        self.parts.fill(UNUSED_PART);
        self.current_index = 0;
        self.clock_start_ns = None;
        self.time_offset = 0.0;
        self.time = 0.0;
        self.last_position = None;
        self.resampler = InputResampler::default();
        self.current_position = None;
        self.last_trail_time = 0.0;
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorTrail.cs (FadeDuration)
    fn fade_duration(disjoint: bool) -> f64 {
        if disjoint { 150.0 } else { 500.0 }
    }

    /// One frame at engine time `now_ns`: takes the cursor's `moves` (osu!pixels, oldest
    /// first) with the part scale of the previous frame, runs the fade clock, then leaves a
    /// disjoint trail's part with `part_scale`. `display_width` is the trail texture's width
    /// in osu!pixels.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/CursorTrail.cs (OnMouseMove, Update, AddTrail)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorTrail.cs (Update, OnMouseMove)
    pub fn update(
        &mut self,
        now_ns: u64,
        moves: impl Iterator<Item = Vec2>,
        disjoint: bool,
        display_width: f32,
        previous_part_scale: f32,
        part_scale: f32,
    ) {
        // Input comes before the frame's update: new parts take the previous fade clock.
        for position in moves {
            if disjoint {
                self.current_position = Some(position);
            } else {
                self.add_trail(position, display_width, previous_part_scale);
            }
        }

        // Lazer's trail has a clock of its own, started when it loads.
        let start = *self.clock_start_ns.get_or_insert(now_ns);
        let current = now_ns.saturating_sub(start) as f64 / 1e6;
        self.time = ((current - self.time_offset) / Self::fade_duration(disjoint)) as f32;
        if self.time > FADE_CLOCK_RESET_THRESHOLD {
            self.reset_time(current);
        }

        if disjoint
            && let Some(position) = self.current_position
            && current - self.last_trail_time >= DISJOINT_TRAIL_TIME_SEPARATION
        {
            self.last_trail_time = current;
            self.add_part(position, part_scale);
        }
    }

    fn reset_time(&mut self, current: f64) {
        for part in &mut self.parts {
            part.time -= self.time;
        }
        self.time = 0.0;
        self.time_offset = current;
    }

    fn add_trail(&mut self, position: Vec2, display_width: f32, part_scale: f32) {
        let Some(pos1) = self.last_position else {
            self.last_position = Some(position);
            self.resampler.add_position(position);
            return;
        };
        let Some(pos2) = self.resampler.add_position(position) else {
            return;
        };
        let diff = pos2 - pos1;
        let distance = diff.length();
        let direction = diff / distance;
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorTrail.cs (IntervalMultiplier, AvoidDrawingNearCursor)
        let interval = display_width * CURSOR_SIZE / 2.5 * (1.0 / CURSOR_SIZE.max(1.0));
        let stop_at = distance - interval;
        let mut d = interval;
        while d < stop_at {
            let at = pos1 + direction * d;
            self.last_position = Some(at);
            self.add_part(at, part_scale);
            d += interval;
        }
    }

    fn add_part(&mut self, position: Vec2, scale: f32) {
        self.parts[self.current_index] = TrailPart {
            position,
            time: self.time + 1.0,
            scale,
            used: true,
        };
        self.current_index = (self.current_index + 1) % MAX_TRAIL_PARTS;
    }

    /// Writes the visible parts, in the order of their slots, rotated by `rotation` degrees.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/CursorTrail.cs (TrailDrawNode.Draw)
    // Ported from osu-resources 2026.1005.0: osu.Game.Resources/Shaders/sh_CursorTrail.vs (main)
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        texture: &SkinSprite,
        config: &CursorSkinConfig,
        rotation: f32,
    ) {
        let angle = if config.trail_rotate {
            rotation.to_radians()
        } else {
            0.0
        };
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorTrail.cs (load)
        let (origin, blend) = if config.disjoint {
            let origin = if config.trail_centre { 0.5 } else { 0.0 };
            (origin, Blend::Alpha)
        } else {
            (0.5, Blend::Additive)
        };
        let display = texture.size / STABLE_MAGIC_SCALE_FACTOR;
        for part in &self.parts {
            if !part.used || self.time - part.time >= 1.0 {
                continue;
            }
            let alpha = (part.time - self.time).clamp(0.0, 1.0).powf(FADE_EXPONENT);
            let scale = part.scale * CURSOR_SIZE * layout.scale();
            let size = [display.x * scale, display.y * scale];
            let at = layout.to_window(part.position);
            batch.push(&Sprite {
                position: [at.x, at.y],
                size,
                origin: [size[0] * origin, size[1] * origin],
                rotation: angle,
                colour: Color {
                    a: alpha,
                    ..Color::WHITE
                },
                texture: texture.texture,
                uv: texture.uv,
                blend,
            });
        }
    }
}

// ---- Cursor ----

/// The draw side of the cursor: the trail and what the next frame needs of this one.
#[derive(Debug, Clone, Default)]
pub struct CursorView {
    trail: CursorTrail,
    /// Path positions taken so far.
    seen: u64,
    /// Restarts of the play the trail belongs to.
    restarts: u32,
    /// The part scale set by the previous frame, for parts added by this frame's input.
    part_scale: Option<f32>,
    /// Game time of the previous frame.
    previous_time: Option<f64>,
}

/// What a frame of the cursor draws from.
#[derive(Debug, Clone, Copy)]
pub struct CursorFrame<'a> {
    pub path: &'a CursorPath,
    pub position: Vec2,
    pub expand: ExpandTimes,
    /// Game time of the frame.
    pub time: f64,
    pub restarts: u32,
    pub now_ns: u64,
}

impl CursorView {
    /// Writes the trail and the cursor of one frame. Hidden until the cursor first moves.
    /// Allocation-free once `batch` has room.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/OsuCursorContainer.cs (Update)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursor.cs (load)
    pub fn push(
        &mut self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        frame: &CursorFrame<'_>,
        textures: &CursorTextures,
    ) {
        if frame.restarts != self.restarts {
            // A new play has a new cursor container.
            self.restarts = frame.restarts;
            self.trail.reset();
            self.part_scale = None;
            self.previous_time = None;
            // Positions of the old play are not the new trail's.
            self.seen = frame.path.written();
        }
        let config = &textures.config;
        let scale = frame.expand.scale(frame.time, config.expand);
        let rotation = rotation(frame.time, config.rotate);
        // The container updates before the cursor's transforms: the trail takes the cursor's
        // scale and rotation of the previous frame (`CompositeDrawable.UpdateSubTree`).
        let lagging = self.previous_time.unwrap_or(frame.time);
        let part_scale = frame.expand.scale(lagging, config.expand);
        let part_rotation = self::rotation(lagging, config.rotate);
        self.previous_time = Some(frame.time);

        let moves = frame.path.since(self.seen);
        self.seen = frame.path.written();
        if let Some(trail) = &textures.trail {
            let display_width = trail.size.x / STABLE_MAGIC_SCALE_FACTOR;
            let previous = self.part_scale.unwrap_or(part_scale);
            self.trail.update(
                frame.now_ns,
                moves,
                config.disjoint,
                display_width,
                previous,
                part_scale,
            );
        }
        self.part_scale = Some(part_scale);

        if frame.path.written() == 0 {
            return;
        }
        if let Some(trail) = &textures.trail {
            self.trail.push(batch, layout, trail, config, part_rotation);
        }
        let at = layout.to_window(frame.position);
        let origin = if config.centre { 0.5 } else { 0.0 };
        let mut piece = |texture: &SkinSprite, scale: f32, rotation: f32| {
            let size = texture.size / STABLE_MAGIC_SCALE_FACTOR * (scale * layout.scale());
            batch.push(&Sprite {
                position: [at.x, at.y],
                size: [size.x, size.y],
                origin: [size.x * origin, size.y * origin],
                rotation: rotation.to_radians(),
                colour: Color::WHITE,
                texture: texture.texture,
                uv: texture.uv,
                blend: Blend::Alpha,
            });
        };
        // Only `cursor` expands and spins (`ExpandTarget`); `cursormiddle` stays on top.
        if let Some(cursor) = &textures.cursor {
            piece(cursor, scale * CURSOR_SIZE, rotation);
        }
        if let Some(middle) = &textures.middle {
            piece(middle, CURSOR_SIZE, 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn press(time: f64, action: Actions, pressed: bool) -> ActionEvent {
        ActionEvent {
            time,
            position: Vec2::ZERO,
            action,
            pressed,
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn used(trail: &CursorTrail) -> Vec<TrailPart> {
        let mut parts: Vec<_> = trail.parts.iter().copied().filter(|p| p.used).collect();
        parts.sort_by(|a, b| a.time.total_cmp(&b.time));
        parts
    }

    #[test]
    fn the_path_gives_the_positions_not_seen_yet() {
        let mut path = CursorPath::default();
        assert_eq!(path.since(0).count(), 0);
        for i in 0..3 {
            path.push(Vec2::new(i as f32, 0.0));
        }
        assert_eq!(path.written(), 3);
        let xs: Vec<f32> = path.since(1).map(|p| p.x).collect();
        assert_eq!(xs, [1.0, 2.0]);
        assert_eq!(path.since(3).count(), 0);
    }

    #[test]
    fn the_path_skips_positions_overwritten() {
        let mut path = CursorPath::default();
        let total = CURSOR_PATH_CAPACITY + 10;
        for i in 0..total {
            path.push(Vec2::new(i as f32, 0.0));
        }
        let xs: Vec<f32> = path.since(0).map(|p| p.x).collect();
        assert_eq!(xs.len(), CURSOR_PATH_CAPACITY);
        assert_eq!(xs[0], 10.0);
        assert_eq!(*xs.last().unwrap(), (total - 1) as f32);
    }

    #[test]
    fn the_cursor_expands_and_contracts_in_100_ms() {
        let never = ExpandTimes::default();
        assert_eq!(never.scale(1000.0, true), 1.0);
        let held = ExpandTimes {
            expanded_at: 0.0,
            contracted_at: f64::NEG_INFINITY,
        };
        assert_eq!(held.scale(0.0, true), 1.0);
        // Easing.Out at a half: 0.75.
        assert!(close(held.scale(50.0, true), 1.225));
        assert!(close(held.scale(100.0, true), 1.3));
        assert!(close(held.scale(5000.0, true), 1.3));
        assert_eq!(held.scale(50.0, false), 1.0);
        // Released halfway: back from 1.225.
        let released = ExpandTimes {
            expanded_at: 0.0,
            contracted_at: 50.0,
        };
        assert!(close(released.scale(50.0, true), 1.225));
        assert!(close(released.scale(100.0, true), 1.225 - 0.225 * 0.75));
        assert!(close(released.scale(150.0, true), 1.0));
        // Pressed and released at once: no expansion shows.
        let tap = ExpandTimes {
            expanded_at: 20.0,
            contracted_at: 20.0,
        };
        assert_eq!(tap.scale(60.0, true), 1.0);
    }

    #[test]
    fn every_press_expands_and_the_last_release_contracts() {
        let mut tracker = ExpandTracker::default();
        tracker.apply(&[
            press(10.0, Actions::LEFT, true),
            press(20.0, Actions::RIGHT, true),
            press(30.0, Actions::LEFT, false),
            press(35.0, Actions::SMOKE, true),
        ]);
        assert_eq!(
            tracker.times(),
            ExpandTimes {
                expanded_at: 20.0,
                contracted_at: f64::NEG_INFINITY,
            }
        );
        tracker.apply(&[
            press(40.0, Actions::RIGHT, false),
            // A release never pressed changes nothing held.
            press(45.0, Actions::LEFT, false),
        ]);
        assert_eq!(tracker.times().contracted_at, 45.0);
        tracker.apply(&[press(50.0, Actions::LEFT, true)]);
        assert_eq!(tracker.times().expanded_at, 50.0);
        tracker.reset();
        assert_eq!(tracker.times(), ExpandTimes::default());
    }

    #[test]
    fn a_release_and_a_press_at_one_time_expand_in_their_order() {
        let mut release_first = ExpandTracker::default();
        release_first.apply(&[
            press(10.0, Actions::LEFT, true),
            press(20.0, Actions::LEFT, false),
            press(20.0, Actions::RIGHT, true),
        ]);
        assert!(close(release_first.times().scale(120.0, true), 1.3));
        let mut press_first = ExpandTracker::default();
        press_first.apply(&[
            press(20.0, Actions::LEFT, true),
            press(20.0, Actions::LEFT, false),
        ]);
        assert_eq!(press_first.times().scale(120.0, true), 1.0);
    }

    #[test]
    fn the_cursor_turns_once_in_ten_seconds() {
        assert!(close(rotation(2500.0, true), 90.0));
        assert!(close(rotation(12500.0, true), 90.0));
        assert!(close(rotation(-2500.0, true), 270.0));
        assert_eq!(rotation(2500.0, false), 0.0);
    }

    #[test]
    fn the_resampler_passes_raw_input_and_reduces_whole_pixels() {
        let mut whole = InputResampler::default();
        assert_eq!(whole.add_position(Vec2::new(0.0, 0.0)), Some(Vec2::ZERO));
        // Under a pixel from the last one.
        assert_eq!(whole.add_position(Vec2::new(0.0, 0.5)), None);
        // Straight on, under 10 pixels.
        assert_eq!(whole.add_position(Vec2::new(5.0, 0.0)), None);
        assert_eq!(
            whole.add_position(Vec2::new(12.0, 0.0)),
            Some(Vec2::new(12.0, 0.0))
        );
        let mut raw = InputResampler::default();
        assert!(raw.add_position(Vec2::new(0.5, 0.0)).is_some());
        assert!(raw.add_position(Vec2::new(0.6, 0.0)).is_some());
    }

    #[test]
    fn a_continuous_trail_leaves_parts_every_interval_short_of_the_cursor() {
        let mut trail = CursorTrail::new();
        // A texture 25 wide: parts every 10.
        let moves = [Vec2::new(0.5, 0.0), Vec2::new(100.5, 0.0)];
        trail.update(0, moves.into_iter(), false, 25.0, 1.0, 1.0);
        let parts = used(&trail);
        // At 10 to 80: the last interval before the cursor stays empty.
        let xs: Vec<f32> = parts.iter().map(|p| p.position.x).collect();
        assert_eq!(xs, [10.5, 20.5, 30.5, 40.5, 50.5, 60.5, 70.5, 80.5]);
        assert_eq!(trail.last_position, Some(Vec2::new(80.5, 0.0)));
        // The next move goes on from the last part.
        trail.update(
            MS,
            [Vec2::new(105.5, 0.0)].into_iter(),
            false,
            25.0,
            1.0,
            1.0,
        );
        assert_eq!(used(&trail).len(), 9);
        assert_eq!(trail.last_position, Some(Vec2::new(90.5, 0.0)));
    }

    #[test]
    fn parts_take_the_fade_clock_and_scale_of_the_previous_frame() {
        let mut trail = CursorTrail::new();
        trail.update(0, [Vec2::new(0.5, 0.0)].into_iter(), false, 25.0, 1.0, 1.0);
        // 250 ms on: half a fade of the continuous trail.
        trail.update(
            250 * MS,
            [Vec2::new(30.5, 0.0)].into_iter(),
            false,
            25.0,
            1.2,
            1.3,
        );
        let parts = used(&trail);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].time, 1.0);
        assert_eq!(parts[0].scale, 1.2);
        assert_eq!(trail.time, 0.5);
    }

    #[test]
    fn parts_are_gone_after_the_fade() {
        let layout = PlayfieldLayout::new(640.0, 480.0);
        let texture = SkinSprite {
            texture: slam_render::textures::TextureId(1),
            uv: slam_render::atlas::UvRect {
                u0: 0.0,
                v0: 0.0,
                u1: 1.0,
                v1: 1.0,
            },
            size: Vec2::new(40.0, 40.0),
        };
        let config = CursorSkinConfig {
            disjoint: false,
            ..CursorSkinConfig::default()
        };
        let mut trail = CursorTrail::new();
        let moves = [Vec2::new(0.5, 0.0), Vec2::new(30.5, 0.0)];
        trail.update(0, moves.into_iter(), false, 25.0, 1.0, 1.0);
        let drawn = |trail: &CursorTrail| {
            let mut batch = SpriteBatch::new();
            trail.push(&mut batch, &layout, &texture, &config, 0.0);
            batch
        };
        let batch = drawn(&trail);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch.batches()[0].blend, Blend::Additive);
        assert_eq!(batch.vertices()[0].colour[3], 1.0);
        trail.update(250 * MS, std::iter::empty(), false, 25.0, 1.0, 1.0);
        assert!(close(drawn(&trail).vertices()[0].colour[3], 0.5));
        // Transparent at the end of the fade, yet drawn for one fade more: lazer's draw node
        // skips a part a whole fade after its alpha reached 0.
        trail.update(500 * MS, std::iter::empty(), false, 25.0, 1.0, 1.0);
        assert_eq!(drawn(&trail).vertices()[0].colour[3], 0.0);
        trail.update(999 * MS, std::iter::empty(), false, 25.0, 1.0, 1.0);
        assert_eq!(drawn(&trail).len(), 1);
        trail.update(1000 * MS, std::iter::empty(), false, 25.0, 1.0, 1.0);
        assert_eq!(drawn(&trail).len(), 0);
    }

    #[test]
    fn a_disjoint_trail_leaves_a_part_sixty_times_a_second() {
        let mut trail = CursorTrail::new();
        let mut now = 0;
        // 1000 Hz frames for 100 ms, the cursor moving every frame.
        while now <= 100 * MS {
            let at = Vec2::new(now as f32 / MS as f32, 0.0);
            trail.update(now, [at].into_iter(), true, 25.0, 1.0, 1.0);
            now += MS;
        }
        let xs: Vec<f32> = used(&trail).iter().map(|p| p.position.x).collect();
        assert_eq!(xs, [17.0, 34.0, 51.0, 68.0, 85.0]);
        // Gone in 150 ms.
        assert!(used(&trail).iter().all(|p| p.time - trail.time < 1.0));
    }

    #[test]
    fn the_fade_clock_starts_over_keeping_the_parts() {
        let mut trail = CursorTrail::new();
        trail.update(0, [Vec2::new(0.5, 0.0)].into_iter(), false, 25.0, 1.0, 1.0);
        trail.update(
            MS,
            [Vec2::new(30.5, 0.0)].into_iter(),
            false,
            25.0,
            1.0,
            1.0,
        );
        let before = used(&trail)[0].time - trail.time;
        // Past a million fade durations.
        let late = 500 * MS * 1_000_001;
        trail.update(late, std::iter::empty(), false, 25.0, 1.0, 1.0);
        assert_eq!(trail.time, 0.0);
        assert_eq!(trail.time_offset, late as f64 / 1e6);
        let after = used(&trail)[0].time;
        assert!(after < before);
    }
}
