//! Gameplay's service elements from the skin (ADR-0140): the countdown before the first
//! object, the warning arrows before a break ends and the section pass or fail in a break.
//!
//! The update side ([`ServiceTracker`]) works out each tick what is on and which sounds play;
//! the snapshot carries a [`ServiceState`]; the draw side ([`ServiceView`]) turns it into
//! sprites at the frame's time.

use slam_osu::beatmap::Beatmap;
use slam_osu::breaks::Break;
use slam_osu::objects::PLAYFIELD_SIZE;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_ui::hud_layout::unit_scale;

use crate::circles::SkinSprite;
use crate::hit_error::LAZER_PIXELS_PER_UNIT;
use crate::playfield::PlayfieldLayout;

/// The countdown's steps, in order: `ready`, `count3`, `count2`, `count1`, `go`.
pub const COUNTDOWN_STEPS: usize = 5;

/// The skin images of the countdown's steps.
pub const COUNTDOWN_IMAGES: [&str; COUNTDOWN_STEPS] = ["ready", "count3", "count2", "count1", "go"];

/// The skin samples of the countdown's steps.
pub const COUNTDOWN_SAMPLES: [&str; COUNTDOWN_STEPS] =
    ["readys", "count3s", "count2s", "count1s", "gos"];

/// The part of its beat a countdown step fades out over, at the end.
const COUNTDOWN_FADE: f64 = 0.25;

/// A step's sound plays if gameplay reaches the step within this part of its beat: after a
/// seek past its start it stays quiet.
const COUNTDOWN_SOUND_WINDOW: f64 = 0.5;

/// The countdown of a play: five steps of one beat each, ending `CountdownOffset` beats
/// before the first object (ADR-0140).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Countdown {
    /// When `ready` shows, in ms.
    pub start: f64,
    /// One step, in ms.
    pub beat: f64,
}

impl Countdown {
    /// The countdown of `beatmap` for a clock starting at `clock_start`, if the beatmap has
    /// one and it fits after the start.
    pub fn of_beatmap(beatmap: &Beatmap, clock_start: f64) -> Option<Countdown> {
        let first = beatmap.hit_objects.first()?.start_time;
        let beat_length = beatmap.control_points.timing_point_at(first).beat_length;
        Countdown::new(
            beatmap.countdown,
            beatmap.countdown_offset,
            first,
            beat_length,
            clock_start,
        )
    }

    /// The countdown of type `kind` (`Countdown`: 1 normal, 2 half speed, 3 double speed;
    /// anything else none), ending `offset` beats before `first_object`, with
    /// `beat_length` ms per beat of the timing point at the first object. `None` if the
    /// whole countdown does not fit after `clock_start`.
    pub fn new(
        kind: i32,
        offset: i32,
        first_object: f64,
        beat_length: f64,
        clock_start: f64,
    ) -> Option<Countdown> {
        let speed = match kind {
            1 => 1.0,
            2 => 2.0,
            3 => 0.5,
            _ => return None,
        };
        let beat = beat_length * speed;
        if !(beat > 0.0 && beat.is_finite()) {
            return None;
        }
        let end = first_object - f64::from(offset) * beat;
        let start = end - COUNTDOWN_STEPS as f64 * beat;
        (start >= clock_start).then_some(Countdown { start, beat })
    }

    /// The step on at `time` and when it started, if any.
    pub fn step_at(&self, time: f64) -> Option<(usize, f64)> {
        if time < self.start {
            return None;
        }
        let index = ((time - self.start) / self.beat) as usize;
        (index < COUNTDOWN_STEPS).then_some((index, self.start + index as f64 * self.beat))
    }

    /// The step shown at `time` and its alpha: opaque, fading out over the last quarter of
    /// its beat.
    pub fn sprite_at(&self, time: f64) -> Option<(usize, f32)> {
        let (index, started) = self.step_at(time)?;
        let left = 1.0 - (time - started) / self.beat;
        Some((index, (left / COUNTDOWN_FADE).clamp(0.0, 1.0) as f32))
    }
}

/// The section pass or fail of a break.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Section {
    /// When the section's display time starts counting, in ms.
    pub start: f64,
    /// `section-pass` (health at least half), else `section-fail`.
    pub pass: bool,
}

// Ported from McOsu db2add2: src/App/Osu/OsuBeatmap.cpp (update: section pass/fail logic)
const SECTION_MIN_GAP: f64 = 2880.0;
const SECTION_FADE_START: f64 = 1280.0;
const SECTION_FADE_END: f64 = 1480.0;
const SECTION_PASS_SOUND: f64 = 20.0;
const SECTION_FAIL_SOUND: f64 = 130.0;

impl Section {
    /// The section's alpha at `time`: two blinks, then shown and faded out, as McOsu does.
    // Ported from McOsu db2add2: src/App/Osu/OsuBeatmap.cpp (update: section pass/fail logic)
    pub fn alpha_at(&self, time: f64) -> f32 {
        let delta = time - self.start;
        if delta <= 0.0 {
            return 0.0;
        }
        let visible = (280.0..=SECTION_FADE_END).contains(&delta)
            || if self.pass {
                (160.0..=230.0).contains(&delta) || (20.0..=100.0).contains(&delta)
            } else {
                (130.0..=230.0).contains(&delta)
            };
        if !visible {
            return 0.0;
        }
        if delta > SECTION_FADE_START {
            (1.0 - (delta - SECTION_FADE_START) / (SECTION_FADE_END - SECTION_FADE_START)) as f32
        } else {
            1.0
        }
    }

    /// How long after [`Self::start`] the sound plays.
    fn sound_delay(&self) -> f64 {
        if self.pass {
            SECTION_PASS_SOUND
        } else {
            SECTION_FAIL_SOUND
        }
    }
}

// Ported from McOsu db2add2: src/App/Osu/OsuBeatmap.cpp (update: warning arrow logic)
const ARROWS_MIN_GAP: f64 = 1000.0;
const ARROWS_LAST_VISIBLE: f64 = 400.0;
const ARROWS_BLINK: f64 = 100.0;
const ARROWS_BLINKS: u32 = 7;
/// The gap after an object counts from this long after its end, as in McOsu.
const ARROWS_LEGACY_OFFSET: f64 = 1000.0;

/// Whether the warning arrows show at `time`, `next` being the next object's start: in the
/// last of seven 100 ms blinks before it (McOsu).
// Ported from McOsu db2add2: src/App/Osu/OsuBeatmap.cpp (update: warning arrow logic)
pub fn arrows_visible(next: f64, time: f64) -> bool {
    let delta = next - time;
    (0..ARROWS_BLINKS).any(|k| {
        let low = ARROWS_LAST_VISIBLE + ARROWS_BLINK * f64::from(2 * k);
        delta > low && delta <= low + ARROWS_BLINK
    })
}

/// What the service elements show, for the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ServiceState {
    pub countdown: Option<Countdown>,
    /// The next object's start while the warning arrows may blink before it.
    pub arrows_before: Option<f64>,
    pub section: Option<Section>,
}

/// A sound of the service elements to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceSound {
    /// The countdown step's sound ([`COUNTDOWN_SAMPLES`]).
    Countdown(usize),
    SectionPass,
    SectionFail,
}

/// The update side of the service elements: what shows and when sounds play.
#[derive(Debug, Clone)]
pub struct ServiceTracker {
    countdown: Option<Countdown>,
    /// Start times of the objects, sorted.
    starts: Vec<f64>,
    /// End times of the objects, in the order of `starts`.
    ends: Vec<f64>,
    /// The latest end of any object.
    last_end: f64,
    breaks: Vec<Break>,
    /// The last countdown step reached.
    step_reached: Option<usize>,
    /// The start of the last section whose sound came due.
    section_sounded: Option<f64>,
}

impl ServiceTracker {
    /// The tracker of `beatmap`'s play with its clock starting at `clock_start`. Allocates.
    pub fn new(beatmap: &Beatmap, clock_start: f64) -> ServiceTracker {
        let objects = &beatmap.hit_objects;
        ServiceTracker {
            countdown: Countdown::of_beatmap(beatmap, clock_start),
            starts: objects.iter().map(|o| o.start_time).collect(),
            ends: objects.iter().map(|o| o.end_time()).collect(),
            last_end: objects
                .iter()
                .map(|o| o.end_time())
                .fold(f64::NEG_INFINITY, f64::max),
            breaks: beatmap.breaks.clone(),
            step_reached: None,
            section_sounded: None,
        }
    }

    /// Forgets the sounds played: the play starts over.
    pub fn reset(&mut self) {
        self.step_reached = None;
        self.section_sounded = None;
    }

    /// The state at `time` with `health` (0..=1), `failed` telling whether the play has
    /// failed, and the sounds that come due, at most two (a countdown step and a section).
    /// Allocation-free.
    pub fn update(
        &mut self,
        time: f64,
        health: f64,
        failed: bool,
    ) -> (ServiceState, [Option<ServiceSound>; 2]) {
        // Gameplay time never goes back within an attempt; a restart calls `reset`.
        let mut sounds = [None, None];

        if let Some(countdown) = self.countdown
            && let Some((index, started)) = countdown.step_at(time)
            && self.step_reached.is_none_or(|reached| index > reached)
        {
            self.step_reached = Some(index);
            if time - started < countdown.beat * COUNTDOWN_SOUND_WINDOW {
                sounds[0] = Some(ServiceSound::Countdown(index));
            }
        }

        let (arrows_before, section) = self.gap_state(time, health, failed);
        if let Some(section) = section {
            let due = section.start + section.sound_delay();
            if time >= due && self.section_sounded != Some(section.start) {
                self.section_sounded = Some(section.start);
                // After a seek far into the section it stays quiet.
                if time - section.start <= SECTION_FADE_END {
                    sounds[1] = Some(if section.pass {
                        ServiceSound::SectionPass
                    } else {
                        ServiceSound::SectionFail
                    });
                }
            }
        }
        (
            ServiceState {
                countdown: self.countdown,
                arrows_before,
                section,
            },
            sounds,
        )
    }

    // Ported from McOsu db2add2: src/App/Osu/OsuBeatmap.cpp (update: previous and next object time, warning arrow logic, section pass/fail logic, getBreakForTimeRange)
    fn gap_state(&self, time: f64, health: f64, failed: bool) -> (Option<f64>, Option<Section>) {
        let Some(&first) = self.starts.first() else {
            return (None, None);
        };
        // The next object starts after `time`; the previous one is the last to start at or
        // before it, and counts until its end (0 before the first object).
        let next_index = self.starts.partition_point(|&start| start <= time);
        let Some(&next) = self.starts.get(next_index) else {
            return (None, None);
        };
        let previous = next_index.checked_sub(1).map_or(0.0, |i| self.ends[i]);

        let legacy_offset = if previous < first {
            0.0
        } else {
            ARROWS_LEGACY_OFFSET
        };
        let arrow_gap = next - (previous + legacy_offset);
        let arrows = (arrow_gap > ARROWS_MIN_GAP && next > time).then_some(next);

        let gap = next - previous;
        let start = if gap / 2.0 > SECTION_MIN_GAP {
            previous + gap / 2.0
        } else {
            next - SECTION_MIN_GAP
        };
        let in_break = self
            .break_between(previous, time, next)
            .is_some_and(|b| time >= b.start && time <= b.end && b.end - b.start > SECTION_MIN_GAP);
        let section = (gap > SECTION_MIN_GAP
            && time > start
            && time > first
            && time < self.last_end
            && !failed
            && in_break)
            .then_some(Section {
                start,
                pass: health >= 0.5,
            });
        (arrows, section)
    }

    /// McOsu's `getBreakForTimeRange`: of the breaks within `from..=to`, the last one taken
    /// while `time` is not before the one taken so far.
    // Ported from McOsu db2add2: src/App/Osu/OsuBeatmap.cpp (getBreakForTimeRange)
    fn break_between(&self, from: f64, time: f64, to: f64) -> Option<Break> {
        let mut found: Option<Break> = None;
        for b in &self.breaks {
            if b.start >= from && b.end <= to && found.is_none_or(|f| time >= f.start) {
                found = Some(*b);
            }
        }
        found
    }
}

/// The skin images of the service elements; missing ones are not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ServiceTextures {
    pub countdown: [Option<SkinSprite>; COUNTDOWN_STEPS],
    pub warning_arrow: Option<SkinSprite>,
    pub section_pass: Option<SkinSprite>,
    pub section_fail: Option<SkinSprite>,
}

/// The most sprites [`ServiceView::push`] writes.
pub const MAX_SERVICE_SPRITES: usize = 6;

// Ported from McOsu db2add2: src/App/Osu/OsuHUD.cpp (drawWarningArrows)
/// Distance of the warning arrows from the window's sides, in McOsu's 640×480 units.
const ARROW_INSET: f32 = 28.0;
/// The arrow's shorter side, in McOsu's 640×480 units, whatever the image's size.
const ARROW_SIZE: f32 = 78.0;
/// The playfield's height is split in this many parts; the arrows sit at 2 and 15. The
/// playfield is lazer's (shifted down 8 osu!pixels), McOsu's is centred: a few pixels apart.
const ARROW_DIVIDER: f32 = 18.0;
const ARROW_ROWS: [f32; 2] = [2.0, 15.0];

/// The draw side of the service elements.
#[derive(Debug, Clone, Copy)]
pub struct ServiceView {
    width: f32,
    height: f32,
    /// Window pixels per skin pixel (lazer's legacy HUD scale).
    sprite_scale: f32,
    /// Window pixels per McOsu 640×480 unit.
    unit: f32,
    playfield_top: f32,
    playfield_height: f32,
}

impl ServiceView {
    /// The view for a window of `width` × `height` pixels.
    pub fn new(width: f32, height: f32) -> ServiceView {
        let unit = unit_scale(width, height);
        let playfield = PlayfieldLayout::new(width, height);
        ServiceView {
            width,
            height,
            sprite_scale: unit / LAZER_PIXELS_PER_UNIT,
            unit,
            playfield_top: playfield.origin().y,
            playfield_height: PLAYFIELD_SIZE.y * playfield.scale(),
        }
    }

    /// Writes the service elements of `state` at game time `time`. Allocation-free once
    /// `batch` has room for [`MAX_SERVICE_SPRITES`].
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        state: &ServiceState,
        time: f64,
        textures: &ServiceTextures,
    ) {
        let centre = [self.width / 2.0, self.height / 2.0];
        if let Some(countdown) = state.countdown
            && let Some((index, alpha)) = countdown.sprite_at(time)
            && let Some(sprite) = textures.countdown[index]
        {
            self.push_centred(batch, sprite, centre, alpha, false);
        }

        if let (Some(next), Some(arrow)) = (state.arrows_before, textures.warning_arrow)
            && arrows_visible(next, time)
        {
            // McOsu fits the arrow to a fixed size (`getImageScale(…, 78)`).
            let fit = ARROW_SIZE * self.unit / arrow.size.x.min(arrow.size.y).max(f32::EPSILON);
            let size = arrow.size.x * fit;
            let part = self.playfield_height / ARROW_DIVIDER;
            let inset = ARROW_INSET * self.unit;
            for row in ARROW_ROWS {
                let y = self.playfield_top + part * row;
                // McOsu places the arrow's outer edge at the inset; the right ones are
                // mirrored.
                self.push_scaled(batch, arrow, fit, [inset + size / 2.0, y], 1.0, false);
                let right = [self.width - inset - size / 2.0, y];
                self.push_scaled(batch, arrow, fit, right, 1.0, true);
            }
        }

        if let Some(section) = state.section {
            let image = if section.pass {
                textures.section_pass
            } else {
                textures.section_fail
            };
            let alpha = section.alpha_at(time);
            if let Some(image) = image
                && alpha > 0.0
            {
                self.push_centred(batch, image, centre, alpha, false);
            }
        }
    }

    /// Draws `image` at the skin's size about `centre`.
    fn push_centred(
        &self,
        batch: &mut SpriteBatch,
        image: SkinSprite,
        centre: [f32; 2],
        alpha: f32,
        mirrored: bool,
    ) {
        self.push_scaled(batch, image, self.sprite_scale, centre, alpha, mirrored);
    }

    /// Draws `image`, `scale` window pixels per display unit, about `centre`.
    fn push_scaled(
        &self,
        batch: &mut SpriteBatch,
        image: SkinSprite,
        scale: f32,
        centre: [f32; 2],
        alpha: f32,
        mirrored: bool,
    ) {
        let size = [image.size.x * scale, image.size.y * scale];
        let mut uv = image.uv;
        if mirrored {
            std::mem::swap(&mut uv.u0, &mut uv.u1);
        }
        let mut sprite = Sprite::new(
            image.texture,
            uv,
            [centre[0] - size[0] / 2.0, centre[1] - size[1] / 2.0],
            size,
        );
        sprite.colour.a = alpha;
        batch.push(&sprite);
    }
}

#[cfg(test)]
mod tests;
