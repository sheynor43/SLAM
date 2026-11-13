//! A play run without a window, an audio device or real time: the track follows the commands
//! on the frame they are given, the output reports its position exactly every tick, and a
//! scripted player hits every object. The same beatmap and time always give the same
//! snapshot.

use std::sync::Arc;

use slam_audio::{
    MusicAnchor, MusicSnapshot, PlayError, PositionSnapshot, Sample, TrackAdjustment, TrackControl,
    VoiceId, VoiceParams,
};
use slam_input::{InputEvent, InputKind};
use slam_osu::objects::OsuHitObjectKind;
use slam_osu::{Beatmap, ModSet, Vec2};
use slam_play::{
    Bindings, GameplaySounds, LiveInput, Offsets, PlaySession, PlaySnapshot, PlayfieldLayout,
    VoiceSink,
};

/// The Escape key's scancode: pauses.
const ESCAPE: u16 = 41;

/// Stream rate of the pretend output.
const RATE: u32 = 48_000;
const MS: u64 = 1_000_000;
/// Engine time of the first output report: the clock starts there, at the play's start time.
const T0: u64 = 1_000 * MS;
/// Bindings the player presses, alternating per object: `Bindings::default`'s Z and X.
const KEYS: [u16; 2] = [29, 27];
/// How long the player holds a key on a circle.
const CIRCLE_HOLD_MS: f64 = 40.0;
/// Radius of the circles the player draws on a spinner, osu!pixels.
const SPIN_RADIUS: f32 = 60.0;
/// Milliseconds per radian the cursor turns on a spinner: about 5 turns a second.
const SPIN_MS_PER_RADIAN: f64 = 30.0;
/// The middle of the playfield, where spinners are.
const CENTRE: Vec2 = Vec2::new(256.0, 192.0);

/// A track that does what it is told at once, at rate `rate`, without frequency changes.
#[derive(Debug, Default)]
struct Track {
    /// The stream frame being written: commands take effect here.
    frame: u64,
    rate: f64,
    playing: bool,
    /// A `play_at` not reached yet.
    scheduled: Option<u64>,
    anchors: Option<(MusicAnchor, MusicAnchor)>,
    generation: u32,
}

impl Track {
    fn new() -> Self {
        Self {
            rate: 1.0,
            ..Self::default()
        }
    }

    fn position(&self, frame: u64) -> f64 {
        self.anchors
            .map_or(0.0, |(_, anchor)| anchor.at(frame as f64))
    }

    /// Starts a stretch at `frame` from `track_frame`.
    fn anchor(&mut self, frame: u64, track_frame: f64) {
        let anchor = MusicAnchor {
            device_frame: frame,
            track_frame,
            speed: if self.playing { self.rate } else { 0.0 },
            accel: 0.0,
        };
        self.anchors = Some(match self.anchors {
            Some((_, last)) => (last, anchor),
            None => (anchor, anchor),
        });
    }

    /// Moves the stream to `frame`, starting a scheduled play on its frame.
    fn advance(&mut self, frame: u64) {
        if let Some(at) = self.scheduled.filter(|&at| at <= frame) {
            self.scheduled = None;
            self.playing = true;
            let track_frame = self.position(at);
            self.anchor(at, track_frame);
        }
        self.frame = frame;
    }

    fn music(&self) -> Option<MusicSnapshot> {
        self.anchors.map(|(previous, anchor)| MusicSnapshot {
            anchor,
            previous,
            generation: self.generation,
            ended: false,
        })
    }

    fn restart_here(&mut self) {
        let (frame, track_frame) = (self.frame, self.position(self.frame));
        self.anchor(frame, track_frame);
    }
}

impl TrackControl for Track {
    fn play(&mut self) -> Result<(), PlayError> {
        self.scheduled = None;
        self.playing = true;
        self.restart_here();
        Ok(())
    }

    fn play_at(&mut self, device_frame: u64) -> Result<(), PlayError> {
        // As `MusicPlayer`: ignored while playing; a frame already passed starts now.
        if !self.playing {
            self.scheduled = Some(device_frame.max(self.frame));
        }
        Ok(())
    }

    fn pause(&mut self) -> Result<(), PlayError> {
        self.scheduled = None;
        self.playing = false;
        self.restart_here();
        Ok(())
    }

    fn seek(&mut self, frame: u64) -> Result<(), PlayError> {
        self.generation += 1;
        self.anchor(self.frame, frame as f64);
        Ok(())
    }

    fn set_rate(&mut self, rate: f64) -> Result<(), PlayError> {
        self.rate = rate;
        self.restart_here();
        Ok(())
    }

    fn adjust(&mut self, _: &[TrackAdjustment]) -> Result<(), PlayError> {
        // Frequency changes (the fail animation) are not followed: the player never fails.
        Ok(())
    }

    fn generation(&self) -> u32 {
        self.generation
    }
}

/// Takes every sound and plays none.
#[derive(Debug, Default)]
struct Silence {
    plays: u64,
}

impl VoiceSink for Silence {
    fn play_voice(&mut self, _: &Arc<Sample>, _: VoiceParams) -> Result<VoiceId, PlayError> {
        self.plays += 1;
        Ok(VoiceId::from_play(self.plays))
    }

    fn stop_voice(&mut self, _: VoiceId) -> Result<(), PlayError> {
        Ok(())
    }

    fn set_voice_balance(&mut self, _: VoiceId, _: f32) -> Result<(), PlayError> {
        Ok(())
    }

    fn set_voice_gain(&mut self, _: VoiceId, _: f32, _: f64) -> Result<(), PlayError> {
        Ok(())
    }

    fn set_voice_rate(&mut self, _: VoiceId, _: f64) -> Result<(), PlayError> {
        Ok(())
    }
}

/// An exact output report at `now_ns`, `None` before the stream starts at `T0`.
fn report(now_ns: u64) -> Option<PositionSnapshot> {
    let elapsed = now_ns.checked_sub(T0)?;
    let frame = elapsed * u64::from(RATE) / 1_000_000_000;
    Some(PositionSnapshot {
        frame,
        // When that frame was written, to the nanosecond.
        timestamp_ns: T0 + frame * 1_000_000_000 / u64::from(RATE),
        latency_ns: 0,
        sample_rate: RATE,
    })
}

/// A play ticked one millisecond at a time by [`OfflinePlay::run_to`].
pub struct OfflinePlay {
    session: PlaySession,
    track: Track,
    sink: Silence,
    snapshot: PlaySnapshot,
    events: Vec<InputEvent>,
    /// Events handed to the session so far.
    next_event: usize,
    now_ns: u64,
}

impl OfflinePlay {
    /// A play of `beatmap` with `mods` on a window of `layout`, with a scripted player
    /// that hits every object on time.
    pub fn new(
        beatmap: &Beatmap,
        mods: &ModSet,
        sounds: GameplaySounds,
        layout: PlayfieldLayout,
    ) -> Self {
        let track = Track::new();
        let input = LiveInput::new(Bindings::default(), layout);
        let session = PlaySession::new(beatmap, mods, &track, input, sounds, Offsets::default());
        let events = autoplay(beatmap, &layout, session.start_time());
        let mut play = Self {
            session,
            track,
            sink: Silence::default(),
            snapshot: PlaySnapshot::default(),
            events,
            next_event: 0,
            // Ticks before the first report wait for the clock.
            now_ns: T0 - 10 * MS,
        };
        play.tick();
        play
    }

    /// Draws the pause menu in `style`, stable's with buttons of `stable` sizes.
    pub fn set_menu_style(
        &mut self,
        style: slam_play::menu_overlay::MenuStyle,
        stable: [[f32; 2]; 3],
    ) {
        self.session
            .set_menu_style(style, slam_ui::theme::ColourScheme::default(), stable);
    }

    /// Presses Escape a quarter of a second before each of `times` (milliseconds of the
    /// beatmap), so each shot shows the pause menu in full.
    pub fn pause_at_each(&mut self, times: &[f64]) {
        let start = self.session.start_time();
        for &time in times {
            let at = engine_ns(time, start).saturating_sub(250 * MS);
            for down in [true, false] {
                let event = InputEvent {
                    time_ns: at + u64::from(!down) * MS,
                    kind: InputKind::Key {
                        scancode: ESCAPE,
                        down,
                    },
                };
                let index = self.events.partition_point(|e| e.time_ns <= event.time_ns);
                self.events.insert(index, event);
            }
        }
    }

    /// Engine time of the last tick.
    pub fn now_ns(&self) -> u64 {
        self.now_ns
    }

    pub fn snapshot(&self) -> &PlaySnapshot {
        &self.snapshot
    }

    /// Engine time of the tick nearest to game time `time` (milliseconds of the beatmap):
    /// ticks are a millisecond apart from the play's start time, so a shot is at most half
    /// a millisecond off and the ticks never depend on the times asked for.
    pub fn target_ns(&self, time: f64) -> u64 {
        engine_ns(
            time.max(self.session.start_time()),
            self.session.start_time(),
        )
    }

    /// Ticks every millisecond up to `target_ns` (a [`OfflinePlay::target_ns`]), calling
    /// `after_tick` after each tick with the play as that tick left it.
    pub fn run_to(&mut self, target_ns: u64, mut after_tick: impl FnMut(&Self)) {
        while self.now_ns < target_ns {
            self.now_ns += MS;
            self.tick();
            after_tick(self);
        }
    }

    fn tick(&mut self) {
        let from = self.next_event;
        while self
            .events
            .get(self.next_event)
            .is_some_and(|e| e.time_ns <= self.now_ns)
        {
            self.next_event += 1;
        }
        let output = report(self.now_ns);
        if let Some(output) = output {
            self.track.advance(output.frame);
        }
        let music = output.and_then(|_| self.track.music());
        self.session.tick(
            self.now_ns,
            &self.events[from..self.next_event],
            output,
            music,
            &mut self.track,
            &mut self.sink,
            &mut self.snapshot,
        );
    }
}

/// Engine time of the tick nearest to game time `time` for a clock started at `T0` from
/// `start_time`: whole milliseconds from `T0`.
fn engine_ns(time: f64, start_time: f64) -> u64 {
    T0.saturating_add_signed((time - start_time).round() as i64 * MS as i64)
}

/// Where the player aims at `time`: on the current object (along a slider's path, in
/// circles on a spinner), otherwise on the way from the last object's end to the next one.
fn target(beatmap: &Beatmap, time: f64) -> Vec2 {
    let objects = &beatmap.hit_objects;
    let Some(i) = objects.iter().position(|o| o.end_time() >= time) else {
        return objects.last().map_or(CENTRE, |o| o.stacked_end_position());
    };
    let object = &objects[i];
    if time < object.start_time {
        let (from, since) = match i.checked_sub(1).map(|p| &objects[p]) {
            Some(previous) => (previous.stacked_end_position(), previous.end_time()),
            None => return object.stacked_position(),
        };
        let to = object.stacked_position();
        let progress = ((time - since) / (object.start_time - since)).clamp(0.0, 1.0) as f32;
        return Vec2::new(
            from.x + (to.x - from.x) * progress,
            from.y + (to.y - from.y) * progress,
        );
    }
    match &object.kind {
        OsuHitObjectKind::Circle => object.stacked_position(),
        OsuHitObjectKind::Slider(slider) => {
            let duration = object.end_time() - object.start_time;
            let progress = if duration > 0.0 {
                ((time - object.start_time) / duration).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let at = slider.curve_position_at(progress);
            let origin = object.stacked_position();
            Vec2::new(origin.x + at.x, origin.y + at.y)
        }
        OsuHitObjectKind::Spinner(_) => {
            let angle = ((time - object.start_time) / SPIN_MS_PER_RADIAN) as f32;
            Vec2::new(
                CENTRE.x + SPIN_RADIUS * angle.cos(),
                CENTRE.y + SPIN_RADIUS * angle.sin(),
            )
        }
    }
}

/// The scripted player's window events in engine time: the cursor every millisecond, and
/// a key held over each object, Z and X in turn. Not lazer's autoplay: it checks drawing,
/// not autoplay's paths, and gets Great on beatmaps whose objects do not overlap.
fn autoplay(beatmap: &Beatmap, layout: &PlayfieldLayout, start_time: f64) -> Vec<InputEvent> {
    let objects = &beatmap.hit_objects;
    let end = objects.last().map_or(start_time, |o| o.end_time()) + 500.0;
    let mut events = Vec::new();
    let mut time = start_time.ceil();
    while time <= end {
        let window = layout.to_window(target(beatmap, time));
        events.push(InputEvent {
            time_ns: engine_ns(time, start_time),
            kind: InputKind::MouseMove {
                x: window.x,
                y: window.y,
            },
        });
        time += 1.0;
    }
    for (i, object) in objects.iter().enumerate() {
        let scancode = KEYS[i % KEYS.len()];
        let release = match object.kind {
            OsuHitObjectKind::Circle => object.start_time + CIRCLE_HOLD_MS,
            _ => object.end_time() + 1.0,
        };
        // Up before the same key is needed again, but never before it went down: objects
        // that overlap the next but one (a slider under two circles) may lose a tick or
        // their tail.
        let release = objects
            .get(i + KEYS.len())
            .map_or(release, |next| release.min(next.start_time - 1.0))
            .max(object.start_time + 1.0);
        for (at, down) in [(object.start_time, true), (release, false)] {
            events.push(InputEvent {
                time_ns: engine_ns(at, start_time),
                kind: InputKind::Key { scancode, down },
            });
        }
    }
    // In time order; at the same time the cursor moves before a key changes.
    events.sort_by_key(|e| (e.time_ns, !matches!(e.kind, InputKind::MouseMove { .. })));
    events
}
