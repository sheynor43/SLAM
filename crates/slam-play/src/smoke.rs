//! Cursor smoke: lazer's `SmokeContainer` and `SmokeSegment` (ADR-0159).
//!
//! Update side: [`SmokeTracker`] records the presses and releases of the smoke key with how
//! far the cursor path had got; they go into the snapshot. Draw side: [`SmokeView`] turns
//! them and the cursor path (ADR-0042) into segments of points that fade by game time.

use slam_osu::Vec2;
use slam_osu::replay::{ActionEvent, Actions};
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};

use crate::circles::SkinSprite;
use crate::cursor::CursorPath;
use crate::playfield::PlayfieldLayout;
use crate::ring::EventRing;

/// Smoke presses and releases a snapshot keeps.
pub const SMOKE_EVENT_CAPACITY: usize = 32;

/// Segments the draw side keeps alive at once; the oldest goes first. A released segment
/// lives at least 8 s: this is over 11 taps a second of the smoke key.
pub const MAX_SMOKE_SEGMENTS: usize = 128;

/// Points of all segments the draw side keeps; the oldest are overwritten.
pub const MAX_SMOKE_POINTS: usize = 16384;

/// A press or release of the smoke key.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SmokeEvent {
    /// Game time of the event.
    pub time: f64,
    /// Cursor position at the event, in osu!pixels.
    pub position: Vec2,
    pub pressed: bool,
    /// Positions written to the cursor path by the end of the event's tick: those after it
    /// come after the event.
    pub path_end: u64,
}

/// The smoke key's presses and releases of the current attempt.
pub type SmokeEvents = EventRing<SmokeEvent, SMOKE_EVENT_CAPACITY>;

/// Follows the smoke key of gameplay's steps.
#[derive(Debug, Clone, Default)]
pub struct SmokeTracker {
    held: bool,
    events: SmokeEvents,
    /// Events of the tick so far, waiting for [`end_tick`](Self::end_tick).
    unsettled: u64,
}

impl SmokeTracker {
    /// Takes a step's presses and releases. Their place on the cursor path is set by
    /// [`end_tick`](Self::end_tick).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/SmokeContainer.cs (OnPressed, OnReleased)
    pub fn apply(&mut self, events: &[ActionEvent]) {
        for event in events {
            if event.action != Actions::SMOKE || event.pressed == self.held {
                continue;
            }
            self.held = event.pressed;
            self.events.push(SmokeEvent {
                time: event.time,
                position: event.position,
                pressed: event.pressed,
                path_end: u64::MAX,
            });
        }
    }

    /// Ends a tick whose steps took the cursor path to `path_end` positions: the tick's
    /// events come before the later ones.
    pub fn end_tick(&mut self, path_end: u64) {
        for index in self.unsettled..self.events.written() {
            if let Some(event) = self.events.get_mut(index) {
                event.path_end = path_end;
            }
        }
        self.unsettled = self.events.written();
    }

    pub fn events(&self) -> &SmokeEvents {
        &self.events
    }

    /// A play that starts over has no smoke of the previous attempt.
    pub fn restart(&mut self) {
        self.held = false;
        self.events.restart();
        self.unsettled = self.events.written();
    }
}

// ---- Draw side ----

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SmokeSegment.cs (constants)
const INITIAL_FADE_OUT_DURATION: f64 = 4000.0;
const RE_FADE_IN_SPEED: f64 = 3.0;
const RE_FADE_IN_DURATION: f64 = 50.0;
const FINAL_FADE_OUT_SPEED: f64 = 2.0;
const FINAL_FADE_OUT_DURATION: f64 = 8000.0;
const INITIAL_ALPHA: f32 = 0.6;
const RE_FADE_IN_ALPHA: f32 = 1.0;
const SCALE_DURATION: f64 = 1200.0;
const INITIAL_SCALE: f32 = 0.65;
const FINAL_SCALE: f32 = 1.0;
const ROTATION_DURATION: f64 = 500.0;
const MAX_ROTATION: f32 = 0.25;
/// A point's half-size over the texture's display size.
const SIZE_FACTOR: f32 = 0.165;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct SmokePoint {
    /// In osu!pixels.
    position: Vec2,
    time: f64,
    angle: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    start: f64,
    /// `f64::MAX` while the key is held, as lazer's `smokeEndTime`.
    end: f64,
    /// Path positions from this one on belong to the segment ...
    path_start: u64,
    /// ... up to this one; `u64::MAX` while held.
    path_end: u64,
    /// Index of the segment's first point among all points written.
    first_point: u64,
    points: u64,
    last_position: Option<Vec2>,
    total_distance: f32,
    /// Lazer's `rotationSeed`.
    seed: u64,
}

impl Segment {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SmokeSegment.cs (FinishDrawing)
    fn lifetime_end(&self) -> f64 {
        let initial_fade_out_duration_trunc = INITIAL_FADE_OUT_DURATION.min(self.end - self.start);
        self.end
            + FINAL_FADE_OUT_DURATION
            + initial_fade_out_duration_trunc / RE_FADE_IN_SPEED
            + initial_fade_out_duration_trunc / FINAL_FADE_OUT_SPEED
    }
}

/// The draw side of the smoke: its segments and points, allocated once.
#[derive(Debug, Clone)]
pub struct SmokeView {
    points: Vec<SmokePoint>,
    points_written: u64,
    segments: [Option<Segment>; MAX_SMOKE_SEGMENTS],
    segments_written: u64,
    /// The segment the held key draws.
    current: Option<usize>,
    /// Smoke events and path positions taken so far.
    seen_events: u64,
    seen_path: u64,
    restarts: u32,
}

impl Default for SmokeView {
    fn default() -> Self {
        SmokeView::new()
    }
}

/// What a frame of the smoke draws from.
#[derive(Debug, Clone, Copy)]
pub struct SmokeFrame<'a> {
    pub events: &'a SmokeEvents,
    pub path: &'a CursorPath,
    /// Game time of the frame.
    pub time: f64,
    pub restarts: u32,
}

impl SmokeView {
    /// No smoke. Allocates its points once.
    pub fn new() -> Self {
        SmokeView {
            points: vec![SmokePoint::default(); MAX_SMOKE_POINTS],
            points_written: 0,
            segments: [None; MAX_SMOKE_SEGMENTS],
            segments_written: 0,
            current: None,
            seen_events: 0,
            seen_path: 0,
            restarts: 0,
        }
    }

    /// Takes the frame's new smoke events and cursor positions, `texture` giving the spacing
    /// of points; without it no points are made. Allocation-free.
    pub fn update(&mut self, frame: &SmokeFrame<'_>, texture: Option<&SkinSprite>) {
        if frame.restarts != self.restarts {
            // A new play has a new smoke container.
            self.restarts = frame.restarts;
            self.segments = [None; MAX_SMOKE_SEGMENTS];
            self.segments_written = 0;
            self.current = None;
            // Positions of the old play are not smoke; those of the new one from its first
            // press on are.
            let first_press = frame
                .events
                .since(self.seen_events)
                .map(|e| e.path_end)
                .next();
            self.seen_path =
                first_press.map_or(frame.path.written(), |end| end.min(frame.path.written()));
        }
        let interval = texture.map(point_interval);
        let mut next = self.seen_path;
        for event in frame.events.since(self.seen_events) {
            // The positions before the event were taken by its time.
            if let Some(interval) = interval {
                let time = frame.time.min(event.time);
                self.take_path(frame.path, &mut next, event.path_end, time, interval);
            }
            if event.pressed {
                self.start(event, interval);
            } else {
                self.finish(event);
            }
        }
        self.seen_events = frame.events.written();
        if let Some(interval) = interval {
            self.take_path(
                frame.path,
                &mut next,
                frame.path.written(),
                frame.time,
                interval,
            );
        }
        self.seen_path = frame.path.written().max(self.seen_path);
        for slot in &mut self.segments {
            if slot.is_some_and(|s| s.end != f64::MAX && frame.time >= s.lifetime_end()) {
                *slot = None;
            }
        }
        if self.current.is_some_and(|i| self.segments[i].is_none()) {
            self.current = None;
        }
    }

    /// Adds the path positions from `*next` up to `end` to the held segment.
    fn take_path(&mut self, path: &CursorPath, next: &mut u64, end: u64, time: f64, interval: f32) {
        while *next < end {
            let index = *next;
            *next += 1;
            let Some(position) = path.at(index) else {
                continue;
            };
            if let Some(i) = self.current
                && let Some(segment) = self.segments[i]
                && index >= segment.path_start
                && index < segment.path_end
            {
                self.add_position(
                    i,
                    position,
                    time.clamp(segment.start, segment.end),
                    interval,
                );
            }
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SmokeSegment.cs (StartDrawing)
    fn start(&mut self, event: &SmokeEvent, interval: Option<f32>) {
        let slot = (self.segments_written % MAX_SMOKE_SEGMENTS as u64) as usize;
        let seed = mix(event.time.to_bits() ^ self.segments_written.rotate_left(40));
        self.segments_written += 1;
        self.segments[slot] = Some(Segment {
            start: event.time,
            end: f64::MAX,
            path_start: event.path_end,
            path_end: u64::MAX,
            first_point: self.points_written,
            points: 0,
            last_position: None,
            total_distance: interval.unwrap_or(0.0),
            seed,
        });
        self.current = Some(slot);
        // Add initial position immediately.
        if let Some(interval) = interval {
            self.add_position(slot, event.position, event.time, interval);
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SmokeSegment.cs (FinishDrawing)
    fn finish(&mut self, event: &SmokeEvent) {
        if let Some(i) = self.current.take()
            && let Some(segment) = &mut self.segments[i]
        {
            segment.end = event.time.max(segment.start);
            segment.path_end = event.path_end;
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SmokeSegment.cs (AddPosition)
    fn add_position(&mut self, slot: usize, position: Vec2, time: f64, interval: f32) {
        let Some(mut segment) = self.segments[slot] else {
            return;
        };
        let last = *segment.last_position.get_or_insert(position);
        // Lazer's `LengthFast` and `NormalizeFast` are approximate; these are exact.
        let delta = (position - last).length();
        segment.total_distance += delta;
        let count = (segment.total_distance / interval) as i32;
        if count > 0 {
            // osuTK's `NormalizeFast` leaves a zero vector zero.
            let mut increment = if delta > 0.0 {
                (position - last) / delta
            } else {
                Vec2::ZERO
            };
            let mut point_position =
                (interval - (segment.total_distance - delta)) * increment + last;
            increment = increment * interval;
            segment.total_distance %= interval;
            let last_time = (segment.points > 0)
                .then(|| self.point(segment.first_point + segment.points - 1).time);
            if last_time.is_none_or(|t| t <= time) {
                for _ in 0..count {
                    let index = segment.points;
                    self.points[(self.points_written % MAX_SMOKE_POINTS as u64) as usize] =
                        SmokePoint {
                            position: point_position,
                            time,
                            angle: unit(segment.seed, index, 0) * 2.0 * std::f32::consts::PI,
                        };
                    self.points_written += 1;
                    segment.points += 1;
                    point_position = point_position + increment;
                }
            }
        }
        segment.last_position = Some(position);
        self.segments[slot] = Some(segment);
    }

    fn point(&self, index: u64) -> SmokePoint {
        self.points[(index % MAX_SMOKE_POINTS as u64) as usize]
    }

    /// Writes the segments' visible points at game time `time`, oldest segment first.
    /// Allocation-free once `batch` has room for [`MAX_SMOKE_POINTS`] sprites.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SmokeSegment.cs (SmokeDrawNode)
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        time: f64,
        texture: &SkinSprite,
    ) {
        let width = texture.size.x * SIZE_FACTOR;
        let height = texture.size.y * SIZE_FACTOR;
        let oldest_point = self.points_written.saturating_sub(MAX_SMOKE_POINTS as u64);
        let first_slot = self
            .segments_written
            .saturating_sub(MAX_SMOKE_SEGMENTS as u64);
        for n in first_slot..self.segments_written {
            let Some(segment) = &self.segments[(n % MAX_SMOKE_SEGMENTS as u64) as usize] else {
                continue;
            };
            if time < segment.start {
                continue;
            }
            let state = SegmentState::new(segment, time);
            let first = segment.first_point.max(oldest_point);
            for index in first..segment.first_point + segment.points {
                let point = self.point(index);
                if point.time < state.first_visible_point_time {
                    continue;
                }
                if point.time > time {
                    break;
                }
                let local = index - segment.first_point;
                let alpha = state.point_alpha(&point);
                if alpha == 0.0 {
                    continue;
                }
                let scale = state.point_scale(&point);
                if scale == 0.0 {
                    continue;
                }
                let angle = state.point_angle(&point, segment.seed, local);
                // Lazer's quad runs along `(sin a, -cos a)` across and `(cos a, sin a)`
                // down: the texture turned by `a - π/2`.
                let size = [
                    2.0 * width * scale * layout.scale(),
                    2.0 * height * scale * layout.scale(),
                ];
                let at = layout.to_window(point.position);
                batch.push(&Sprite {
                    position: [at.x, at.y],
                    size,
                    origin: [size[0] / 2.0, size[1] / 2.0],
                    rotation: angle - std::f32::consts::FRAC_PI_2,
                    colour: Color {
                        a: alpha,
                        ..Color::WHITE
                    },
                    texture: texture.texture,
                    uv: texture.uv,
                    blend: Blend::Additive,
                });
            }
        }
    }

    /// Points alive (written and not overwritten) of all segments; for tests.
    pub fn point_count(&self) -> u64 {
        let oldest_point = self.points_written.saturating_sub(MAX_SMOKE_POINTS as u64);
        self.segments
            .iter()
            .flatten()
            .map(|s| (s.first_point + s.points).saturating_sub(s.first_point.max(oldest_point)))
            .sum()
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SmokeSegment.cs (pointInterval, width)
fn point_interval(texture: &SkinSprite) -> f32 {
    texture.size.x * SIZE_FACTOR * 7.0 / 8.0
}

/// A segment's animation values at one frame: lazer's `SmokeDrawNode.ApplyState`.
#[derive(Debug, Clone, Copy)]
struct SegmentState {
    current_time: f64,
    first_visible_point_time_after_smoke_ended: f64,
    initial_fade_out_time: f64,
    re_fade_in_time: f64,
    final_fade_out_time: f64,
    first_visible_point_time: f64,
}

impl SegmentState {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SmokeSegment.cs (SmokeDrawNode.ApplyState)
    fn new(segment: &Segment, current_time: f64) -> Self {
        let initial_fade_out_duration_trunc =
            INITIAL_FADE_OUT_DURATION.min(segment.end - segment.start);
        let first_visible_point_time_after_smoke_ended =
            segment.end - initial_fade_out_duration_trunc;
        SegmentState {
            current_time,
            first_visible_point_time_after_smoke_ended,
            initial_fade_out_time: current_time.min(segment.end),
            re_fade_in_time: current_time
                - initial_fade_out_duration_trunc
                - first_visible_point_time_after_smoke_ended * (1.0 - 1.0 / RE_FADE_IN_SPEED),
            final_fade_out_time: current_time
                - initial_fade_out_duration_trunc
                - first_visible_point_time_after_smoke_ended * (1.0 - 1.0 / FINAL_FADE_OUT_SPEED),
            first_visible_point_time: segment.end.min(current_time)
                - initial_fade_out_duration_trunc,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SmokeSegment.cs (PointColour)
    fn point_alpha(&self, point: &SmokePoint) -> f32 {
        let mut alpha = 1.0f32;
        let time_doing_final_fade_out =
            self.final_fade_out_time - point.time / FINAL_FADE_OUT_SPEED;
        if time_doing_final_fade_out > 0.0
            && point.time >= self.first_visible_point_time_after_smoke_ended
        {
            let fraction =
                ((time_doing_final_fade_out / FINAL_FADE_OUT_DURATION) as f32).clamp(0.0, 1.0);
            let fraction = fraction.powf(5.0);
            alpha = (1.0 - fraction) * RE_FADE_IN_ALPHA;
        } else {
            let time_doing_initial_fade_out = self.initial_fade_out_time - point.time;
            if time_doing_initial_fade_out > 0.0 {
                let fraction = ((time_doing_initial_fade_out / INITIAL_FADE_OUT_DURATION) as f32)
                    .clamp(0.0, 1.0);
                alpha = (1.0 - fraction) * INITIAL_ALPHA;
            }
            if point.time > self.first_visible_point_time_after_smoke_ended {
                let time_doing_re_fade_in = self.re_fade_in_time - point.time / RE_FADE_IN_SPEED;
                if time_doing_re_fade_in > 0.0 {
                    let fraction =
                        ((time_doing_re_fade_in / RE_FADE_IN_DURATION) as f32).clamp(0.0, 1.0);
                    let fraction = 1.0 - (1.0 - fraction).powf(5.0);
                    alpha = fraction * (RE_FADE_IN_ALPHA - alpha) + alpha;
                }
            }
        }
        alpha
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SmokeSegment.cs (PointScale)
    fn point_scale(&self, point: &SmokePoint) -> f32 {
        let time_doing_scale = self.current_time - point.time;
        let fraction = ((time_doing_scale / SCALE_DURATION) as f32).clamp(0.0, 1.0);
        let fraction = 1.0 - (1.0 - fraction).powf(5.0);
        fraction * (FINAL_SCALE - INITIAL_SCALE) + INITIAL_SCALE
    }

    /// The point's angle in radians: its own plus its turn so far.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SmokeSegment.cs (PointDirection, getRotation)
    fn point_angle(&self, point: &SmokePoint, seed: u64, index: u64) -> f32 {
        let time_doing_rotation = self.current_time - point.time;
        let fraction = ((time_doing_rotation / ROTATION_DURATION) as f32).clamp(0.0, 1.0);
        let fraction = 1.0 - (1.0 - fraction).powf(5.0);
        let rotation = MAX_ROTATION * (unit(seed, index, 1) * 2.0 - 1.0);
        fraction * rotation + point.angle
    }
}

/// SplitMix64's finaliser.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A number in `[0, 1)` for point `index` of a segment of `seed`, one sequence per
/// `stream`: lazer's `RNG.NextSingle` and `StatelessRNG.NextSingle` from a deterministic
/// source (ADR-0043).
fn unit(seed: u64, index: u64, stream: u64) -> f32 {
    let z = mix(seed ^ mix(index.wrapping_mul(2).wrapping_add(stream)));
    // The top 24 bits: every value exact in `f32`, never 1.
    (z >> 40) as f32 / (1u64 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(time: f64, pressed: bool, path_end: u64) -> SmokeEvent {
        SmokeEvent {
            time,
            position: Vec2::ZERO,
            pressed,
            path_end,
        }
    }

    fn segment(start: f64, end: f64) -> Segment {
        Segment {
            start,
            end,
            path_start: 0,
            path_end: u64::MAX,
            first_point: 0,
            points: 0,
            last_position: None,
            total_distance: 0.0,
            seed: 1,
        }
    }

    fn point(time: f64) -> SmokePoint {
        SmokePoint {
            position: Vec2::ZERO,
            time,
            angle: 0.0,
        }
    }

    #[test]
    fn a_held_segment_fades_from_full_to_0_6_and_out_over_4_s() {
        let held = segment(0.0, f64::MAX);
        let p = point(1000.0);
        // Just made: no fade has begun.
        assert_eq!(SegmentState::new(&held, 1000.0).point_alpha(&p), 1.0);
        let alpha = SegmentState::new(&held, 2000.0).point_alpha(&p);
        assert!((alpha - 0.45).abs() < 1e-6, "{alpha}");
        assert_eq!(SegmentState::new(&held, 5000.0).point_alpha(&p), 0.0);
    }

    #[test]
    fn released_smoke_comes_back_then_fades_out() {
        // Held 0..3000: every point is within the last 4 s and comes back.
        let done = segment(0.0, 3000.0);
        let p = point(1000.0);
        // At release: the initial fade so far, (1 - 2000/4000) · 0.6.
        let at_release = SegmentState::new(&done, 3000.0).point_alpha(&p);
        assert!((at_release - 0.3).abs() < 1e-6, "{at_release}");
        // The re-fade-in of point t starts at 3000 + t / 3: fully back 50 ms on.
        let back = SegmentState::new(&done, 3000.0 + 1000.0 / 3.0 + 50.0).point_alpha(&p);
        assert!((back - 1.0).abs() < 1e-6, "{back}");
        // The final fade of point t starts at 3000 + t / 2 and lasts 8 s (power 5).
        let start = 3000.0 + 1000.0 / 2.0;
        let half = SegmentState::new(&done, start + 4000.0).point_alpha(&p);
        assert!((half - (1.0 - 0.5f32.powf(5.0))).abs() < 1e-6, "{half}");
        assert_eq!(
            SegmentState::new(&done, start + 8000.0).point_alpha(&p),
            0.0
        );
        assert!(done.lifetime_end() >= start + 8000.0);
    }

    #[test]
    fn points_grow_and_turn_out_quint() {
        let held = segment(0.0, f64::MAX);
        let p = point(0.0);
        assert_eq!(SegmentState::new(&held, 0.0).point_scale(&p), INITIAL_SCALE);
        assert_eq!(
            SegmentState::new(&held, 1200.0).point_scale(&p),
            FINAL_SCALE
        );
        let half = SegmentState::new(&held, 600.0).point_scale(&p);
        let expected = (1.0 - 0.5f32.powf(5.0)) * (FINAL_SCALE - INITIAL_SCALE) + INITIAL_SCALE;
        assert!((half - expected).abs() < 1e-6);
        let turn = SegmentState::new(&held, 500.0).point_angle(&p, 7, 3);
        assert!(turn.abs() <= MAX_ROTATION);
        assert_eq!(turn, SegmentState::new(&held, 900.0).point_angle(&p, 7, 3));
    }

    #[test]
    fn the_tracker_records_smoke_alone_with_the_path_count() {
        let mut tracker = SmokeTracker::default();
        let press = |action, pressed| ActionEvent {
            time: 10.0,
            position: Vec2::new(1.0, 2.0),
            action,
            pressed,
        };
        tracker.apply(&[press(Actions::LEFT, true), press(Actions::SMOKE, true)]);
        tracker.end_tick(5);
        tracker.apply(&[press(Actions::SMOKE, false)]);
        tracker.end_tick(9);
        let events: Vec<_> = tracker.events().since(0).copied().collect();
        assert_eq!(events.len(), 2);
        assert!(events[0].pressed && events[0].path_end == 5);
        assert!(!events[1].pressed && events[1].path_end == 9);
        tracker.restart();
        assert_eq!(tracker.events().since(0).count(), 0);
    }

    #[test]
    fn press_and_release_in_one_frame_take_only_the_positions_between() {
        let tex = texture();
        let interval = point_interval(&tex);
        let mut path = CursorPath::default();
        let mut events = SmokeEvents::default();
        // Two segments in one frame, with moves before, between and after.
        for (start, pressed_at, released_at) in [(0.0, 10.0, 20.0), (200.0, 30.0, 40.0)] {
            path.push(Vec2::new(start - 50.0, 0.0));
            events.push(SmokeEvent {
                time: pressed_at,
                position: Vec2::new(start, 0.0),
                pressed: true,
                path_end: path.written(),
            });
            for i in 1..=50 {
                path.push(Vec2::new(start + i as f32, 0.0));
            }
            events.push(event(released_at, false, path.written()));
            path.push(Vec2::new(start + 100.0, 0.0));
        }
        let mut view = SmokeView::new();
        view.update(
            &SmokeFrame {
                events: &events,
                path: &path,
                time: 50.0,
                restarts: 0,
            },
            Some(&tex),
        );
        let per_segment = 1 + (50.0 / interval) as u64;
        assert_eq!(view.point_count(), 2 * per_segment);
        // Each point keeps its segment's time span.
        let times: Vec<f64> = (0..view.points_written)
            .map(|i| view.point(i).time)
            .collect();
        assert!(
            times[..per_segment as usize]
                .iter()
                .all(|&t| t == 10.0 || t == 20.0)
        );
        assert!(
            times[per_segment as usize..]
                .iter()
                .all(|&t| t == 30.0 || t == 40.0)
        );
    }

    fn texture() -> SkinSprite {
        SkinSprite {
            texture: slam_render::textures::TextureId(1),
            uv: slam_render::atlas::UvRect {
                u0: 0.0,
                v0: 0.0,
                u1: 1.0,
                v1: 1.0,
            },
            size: Vec2::new(32.0, 32.0),
        }
    }

    #[test]
    fn a_segment_takes_its_own_positions_one_interval_apart() {
        let tex = texture();
        let interval = point_interval(&tex);
        let mut path = CursorPath::default();
        let mut events = SmokeEvents::default();
        // A move before the press is not smoke.
        path.push(Vec2::new(-50.0, 0.0));
        events.push(event(0.0, true, path.written()));
        for i in 1..=100 {
            path.push(Vec2::new(i as f32, 0.0));
        }
        let mut view = SmokeView::new();
        view.update(
            &SmokeFrame {
                events: &events,
                path: &path,
                time: 10.0,
                restarts: 0,
            },
            Some(&tex),
        );
        // The press point and one every interval along 100 osu!pixels.
        let expected = 1 + (100.0 / interval) as u64;
        assert_eq!(view.point_count(), expected);
        // Released: later moves are not this segment's.
        events.push(event(20.0, false, path.written()));
        for i in 1..=100 {
            path.push(Vec2::new(100.0, i as f32));
        }
        view.update(
            &SmokeFrame {
                events: &events,
                path: &path,
                time: 30.0,
                restarts: 0,
            },
            Some(&tex),
        );
        assert_eq!(view.point_count(), expected);
        // Gone once the fades are over.
        view.update(
            &SmokeFrame {
                events: &events,
                path: &path,
                time: 20_000.0,
                restarts: 0,
            },
            Some(&tex),
        );
        assert_eq!(view.point_count(), 0);
    }

    #[test]
    fn long_smoke_keeps_the_newest_points() {
        let tex = texture();
        let mut path = CursorPath::default();
        let mut events = SmokeEvents::default();
        events.push(event(0.0, true, 0));
        let mut view = SmokeView::new();
        let mut x = 0.0;
        for frame in 0..400 {
            for _ in 0..200 {
                x += 1.0;
                path.push(Vec2::new(x, 0.0));
            }
            view.update(
                &SmokeFrame {
                    events: &events,
                    path: &path,
                    time: f64::from(frame),
                    restarts: 0,
                },
                Some(&tex),
            );
        }
        assert_eq!(view.point_count(), MAX_SMOKE_POINTS as u64);
        let mut batch = SpriteBatch::with_capacity(MAX_SMOKE_POINTS);
        let layout = PlayfieldLayout::new(800.0, 600.0);
        view.push(&mut batch, &layout, 400.0, &tex);
        assert!(batch.len() <= MAX_SMOKE_POINTS);
        assert!(!batch.is_empty());
    }

    #[test]
    fn a_restart_drops_the_smoke() {
        let tex = texture();
        let mut path = CursorPath::default();
        let mut events = SmokeEvents::default();
        events.push(event(0.0, true, 0));
        path.push(Vec2::new(10.0, 0.0));
        let mut view = SmokeView::new();
        let frame = |restarts| SmokeFrame {
            events: &events,
            path: &path,
            time: 5.0,
            restarts,
        };
        view.update(&frame(0), Some(&tex));
        assert!(view.point_count() > 0);
        view.update(&frame(1), Some(&tex));
        assert_eq!(view.point_count(), 0);
    }
}
