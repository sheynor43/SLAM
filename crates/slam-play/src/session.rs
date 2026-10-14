//! A play from start to end: the update side runs the game clock, live input, rules and
//! sounds and writes a [`PlaySnapshot`]; the draw side turns the snapshot into sprites.
//!
//! No pause, breaks, skip, failing or HUD yet: the clock starts once the output stream
//! reports its position and runs until the window closes.

use slam_audio::{ClockError, GameClock, MusicSnapshot, PositionSnapshot, TrackControl};
use slam_input::InputEvent;
use slam_osu::judgement::JudgementLog;
use slam_osu::replay::Rules;
use slam_osu::rules::OsuRules;
use slam_osu::{Beatmap, ModSet, Vec2};
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::upload::{
    TextureHandle, TextureKind, TextureRequests, TextureState, TextureUploads,
};
use slam_skin::SkinChain;

use crate::circles::{CircleTextures, HitCircles, LiveCircles, SkinSprite};
use crate::gameplay_sounds::{GameplaySounds, VoiceSink};
use crate::input::{ClockTime, GameTime, LiveInput};
use crate::playfield::PlayfieldLayout;
use crate::time::Offsets;

/// Lead-in before the first object appears. A stand-in until lazer's lead-in
/// (`AudioLeadIn`, `GameplayClockContainer` start time) is ported.
const MIN_LEAD_IN_MS: f64 = 1000.0;

/// Where a play is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    /// Waiting for the first output position report to start the clock.
    Waiting,
    Playing,
    /// Every object is judged.
    Finished,
}

/// What the draw side needs of one update tick. Fixed size, copied into a buffer slot with
/// `clone_from` without allocations.
#[derive(Debug, Clone)]
pub struct PlaySnapshot {
    pub state: PlayState,
    /// Game time of the tick, in milliseconds.
    pub time: f64,
    /// The tick's clock, to place a frame between ticks. `None` while waiting.
    pub clock: Option<ClockTime>,
    /// Cursor position in osu!pixels.
    pub cursor: Vec2,
    pub circles: LiveCircles,
}

impl Default for PlaySnapshot {
    fn default() -> Self {
        PlaySnapshot {
            state: PlayState::Waiting,
            time: 0.0,
            clock: None,
            cursor: Vec2::ZERO,
            circles: LiveCircles::new(),
        }
    }
}

impl PlaySnapshot {
    /// Game time at engine time `now_ns` (a frame): the clock's time, not before the tick's.
    pub fn time_at(&self, now_ns: u64) -> f64 {
        self.clock
            .map_or(self.time, |clock| clock.game_time(now_ns).max(self.time))
    }
}

/// The update side of a play.
#[derive(Debug)]
pub struct PlaySession {
    clock: GameClock,
    offsets: Offsets,
    start_time: f64,
    started: bool,
    input: LiveInput,
    rules: OsuRules,
    log: JudgementLog,
    sounds: GameplaySounds,
    /// Clock commands that failed (queue full); retried on the next tick.
    clock_errors: u64,
}

impl PlaySession {
    /// A play of `beatmap` (with `mods` applied) on the track `track` controls. Call it off
    /// the hot paths.
    pub fn new(
        beatmap: &Beatmap,
        mods: &ModSet,
        track: &impl TrackControl,
        input: LiveInput,
        sounds: GameplaySounds,
        offsets: Offsets,
    ) -> PlaySession {
        let rules = OsuRules::new(beatmap, mods);
        let log = JudgementLog::with_capacity(rules.max_judgements());
        PlaySession {
            clock: GameClock::new(track),
            offsets,
            start_time: start_time(beatmap),
            started: false,
            input,
            rules,
            log,
            sounds,
            clock_errors: 0,
        }
    }

    /// Game time the clock starts at: before the first object's fade-in.
    pub fn start_time(&self) -> f64 {
        self.start_time
    }

    pub fn log(&self) -> &JudgementLog {
        &self.log
    }

    pub fn input(&self) -> &LiveInput {
        &self.input
    }

    /// Clock commands that failed and were retried.
    pub fn clock_errors(&self) -> u64 {
        self.clock_errors
    }

    /// Changes the playfield layout of the input (the window was resized).
    pub fn set_layout(&mut self, layout: PlayfieldLayout) {
        self.input.set_layout(layout);
    }

    /// One update tick at engine time `now_ns` with the tick's input `events` and the latest
    /// reads of the output and music positions. Overwrites all of `snapshot`.
    /// Allocation-free.
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        now_ns: u64,
        events: &[InputEvent],
        output: Option<PositionSnapshot>,
        music: Option<MusicSnapshot>,
        track: &mut impl TrackControl,
        sink: &mut impl VoiceSink,
        snapshot: &mut PlaySnapshot,
    ) {
        let clock = self.clock.update(now_ns, output, music);
        let clock = match clock {
            Some(clock) if self.started => clock,
            Some(_) => {
                // The first tick with a position: the clock can take commands. Input before
                // the play starts is dropped.
                match self.start(now_ns, track) {
                    Ok(()) => self.started = true,
                    Err(_) => self.clock_errors += 1,
                }
                return waiting(snapshot, self.start_time, &self.input);
            }
            None => return waiting(snapshot, self.start_time, &self.input),
        };

        let time = ClockTime {
            clock,
            offsets: self.offsets,
        };
        let step = self.input.step(events, now_ns, &time);
        let step_time = step.time;
        self.rules.update(&step, &mut self.log);
        self.sounds.update(step_time, &self.log, &self.rules, sink);

        snapshot.state = if self.rules.is_complete() {
            PlayState::Finished
        } else {
            PlayState::Playing
        };
        snapshot.time = step_time;
        snapshot.clock = Some(time);
        snapshot.cursor = self.input.position();
        snapshot.circles.capture(&self.rules);
    }

    /// Stops the looping sounds (the play is closing). Returns whether all stops were
    /// queued; call again until it does.
    pub fn stop_sounds(&mut self, sink: &mut impl VoiceSink) -> bool {
        self.sounds.stop_all(sink)
    }

    fn start(&mut self, now_ns: u64, track: &mut impl TrackControl) -> Result<(), ClockError> {
        // Game time includes the offsets; the clock runs in track time. Rate 1: the clock
        // is new and no rate mods exist yet (take the clock's rate once they do).
        let track_time = crate::time::track_time_for(self.start_time, 1.0, &self.offsets);
        self.clock.seek(now_ns, track_time, track)?;
        self.clock.start(now_ns, track)
    }
}

fn waiting(snapshot: &mut PlaySnapshot, start_time: f64, input: &LiveInput) {
    snapshot.state = PlayState::Waiting;
    snapshot.time = start_time;
    snapshot.clock = None;
    snapshot.cursor = input.position();
    snapshot.circles.clear();
}

/// The time a play starts at: 0, or earlier so that the first object's fade-in starts
/// [`MIN_LEAD_IN_MS`] after the start.
fn start_time(beatmap: &Beatmap) -> f64 {
    beatmap.hit_objects.first().map_or(0.0, |first| {
        (first.start_time - first.defaults.time_preempt - MIN_LEAD_IN_MS).min(0.0)
    })
}

// ---- Draw side ----

/// Skin textures of a play, by element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Element {
    Circle,
    Overlay,
    Approach,
    Cursor,
    Digit(u8),
}

const ELEMENTS: [Element; 14] = [
    Element::Circle,
    Element::Overlay,
    Element::Approach,
    Element::Cursor,
    Element::Digit(0),
    Element::Digit(1),
    Element::Digit(2),
    Element::Digit(3),
    Element::Digit(4),
    Element::Digit(5),
    Element::Digit(6),
    Element::Digit(7),
    Element::Digit(8),
    Element::Digit(9),
];

/// A requested skin texture with the pixel density of its file.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Requested {
    handle: TextureHandle,
    scale: f32,
}

/// Why skin textures could not be requested.
#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("{path}: {source}")]
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("texture request rejected: {0}")]
    Rejected(#[from] slam_render::upload::Rejected),
}

/// The skin textures of a play: requested once, looked up every frame.
#[derive(Debug, Clone)]
pub struct PlayTextures {
    requested: [Option<Requested>; ELEMENTS.len()],
}

impl PlayTextures {
    /// Reads the textures of hit circles and the cursor from `skins` and queues them for
    /// upload. Missing ones are not drawn. Does file I/O: call it off the hot paths.
    pub fn request(
        skins: &SkinChain,
        requests: &mut TextureRequests,
    ) -> Result<PlayTextures, RequestError> {
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs (GetFontPrefix)
        let prefix = skins
            .config(|s| s.setting("HitCirclePrefix").map(str::to_owned))
            .map_or_else(|| "default".to_owned(), |f| f.value);
        let mut requested = [None; ELEMENTS.len()];
        for (slot, element) in requested.iter_mut().zip(ELEMENTS) {
            let name = match element {
                Element::Circle => "hitcircle".to_owned(),
                Element::Overlay => "hitcircleoverlay".to_owned(),
                Element::Approach => "approachcircle".to_owned(),
                Element::Cursor => "cursor".to_owned(),
                Element::Digit(d) => format!("{prefix}-{d}"),
            };
            let Some(found) = skins.texture(&name) else {
                continue;
            };
            let path = &found.value.path;
            let bytes = std::fs::read(path).map_err(|source| RequestError::Read {
                path: path.clone(),
                source,
            })?;
            let handle = requests.load(bytes, TextureKind::Atlas)?;
            *slot = Some(Requested {
                handle,
                scale: found.value.scale,
            });
        }
        Ok(PlayTextures { requested })
    }

    /// The textures uploaded by this frame; pending and failed ones are `None`.
    pub fn resolve(&self, uploads: &TextureUploads) -> FrameTextures {
        let mut frame = FrameTextures::default();
        for (requested, element) in self.requested.iter().zip(ELEMENTS) {
            let Some(r) = requested else { continue };
            let TextureState::Ready(region) = uploads.get(r.handle) else {
                continue;
            };
            match element {
                Element::Circle => frame.circles.circle = Some(SkinSprite::circle(region, r.scale)),
                Element::Overlay => {
                    frame.circles.overlay = Some(SkinSprite::circle(region, r.scale))
                }
                Element::Approach => {
                    frame.circles.approach = Some(SkinSprite::circle(region, r.scale))
                }
                Element::Cursor => frame.cursor = Some(SkinSprite::new(region, r.scale, f32::MAX)),
                Element::Digit(d) => {
                    frame.circles.digits[usize::from(d)] = Some(SkinSprite::digit(region, r.scale))
                }
            }
        }
        frame
    }

    /// Requests whose file was found in the skins.
    pub fn found(&self) -> usize {
        self.requested.iter().flatten().count()
    }
}

/// The textures of one frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameTextures {
    pub circles: CircleTextures,
    pub cursor: Option<SkinSprite>,
}

/// The draw side of a play.
#[derive(Debug, Clone)]
pub struct PlayView {
    pub circles: HitCircles,
}

impl PlayView {
    /// Writes the sprites of `snapshot` drawn at engine time `now_ns` on a surface laid out
    /// by `layout`. Allocation-free once `batch` has room.
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        snapshot: &PlaySnapshot,
        now_ns: u64,
        textures: &FrameTextures,
    ) {
        if snapshot.state != PlayState::Waiting {
            let time = snapshot.time_at(now_ns);
            self.circles.push(
                batch,
                layout,
                snapshot.circles.as_slice(),
                time,
                &textures.circles,
            );
        }
        // A stand-in for the skinned cursor and its trail.
        if let Some(t) = &textures.cursor {
            let centre = layout.to_window(snapshot.cursor);
            let size = [t.size.x * layout.scale(), t.size.y * layout.scale()];
            let mut sprite = Sprite::new(t.texture, t.uv, [centre.x, centre.y], size);
            sprite.origin = [size[0] / 2.0, size[1] / 2.0];
            batch.push(&sprite);
        }
    }
}
