//! Game clock (ADR-0035): maps engine time to the track time being heard.
//!
//! Position reports are smoothed into an output-frame axis (which stream frame leaves the
//! device when); the track time is the music position at that frame. Off the track
//! (negative lead-in time, after the end) time runs on the same axis, and the track is
//! started with [`TrackControl::play_at`] on the exact frame where time reaches 0.

use crate::{MusicPlayer, MusicSnapshot, PlayError, PositionSnapshot, TrackAdjustment};

/// A report further than this from the model resets it (xrun, device hiccup).
const RESET_ERROR_NS: f64 = 8e6;
/// The model's remaining error decays with this time constant.
const CORRECTION_NS: f64 = 2e8;
/// Largest relative slope change while correcting the model.
const MAX_SLEW: f64 = 0.01;
/// Without a newer report, the output axis stops this long after the last one: a
/// stalled device stops time instead of running it on without sound.
const STALE_NS: u64 = 250_000_000;

/// Track commands the clock issues. Implemented by [`MusicPlayer`]; tests substitute it.
pub trait TrackControl {
    fn play(&mut self) -> Result<(), PlayError>;
    /// Plays from stream frame `device_frame` on (see [`MusicPlayer::play_at`]).
    fn play_at(&mut self, device_frame: u64) -> Result<(), PlayError>;
    fn pause(&mut self) -> Result<(), PlayError>;
    /// Moves to track frame `frame` at the stream rate, keeping play or pause.
    fn seek(&mut self, frame: u64) -> Result<(), PlayError>;
    /// Sets the track rate (see [`MusicPlayer::set_rate`]).
    fn set_rate(&mut self, rate: f64) -> Result<(), PlayError>;
    /// Changes how the track sounds, all or nothing (see [`MusicPlayer::adjust`]). The
    /// clock follows the heard position, so a frequency change moves time with it.
    fn adjust(&mut self, adjustments: &[TrackAdjustment]) -> Result<(), PlayError>;
    /// Generation of the last load or seek, as in [`MusicSnapshot::generation`].
    fn generation(&self) -> u32;
}

impl TrackControl for MusicPlayer {
    fn play(&mut self) -> Result<(), PlayError> {
        MusicPlayer::play(self)
    }

    fn play_at(&mut self, device_frame: u64) -> Result<(), PlayError> {
        MusicPlayer::play_at(self, device_frame)
    }

    fn pause(&mut self) -> Result<(), PlayError> {
        MusicPlayer::pause(self)
    }

    fn seek(&mut self, frame: u64) -> Result<(), PlayError> {
        MusicPlayer::seek(self, frame)
    }

    fn set_rate(&mut self, rate: f64) -> Result<(), PlayError> {
        MusicPlayer::set_rate(self, rate)
    }

    fn adjust(&mut self, adjustments: &[TrackAdjustment]) -> Result<(), PlayError> {
        MusicPlayer::adjust(self, adjustments)
    }

    fn generation(&self) -> u32 {
        MusicPlayer::generation(self)
    }
}

/// Why a clock command failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClockError {
    /// No position report yet: the output stream has not run.
    #[error("no audio output position yet")]
    NoOutput,
    /// A track command was not queued. Commands sent before it in the same call stay
    /// queued, and the clock follows them; see the method.
    #[error(transparent)]
    Track(#[from] PlayError),
    /// A seek target that is not finite.
    #[error("seek time must be finite")]
    InvalidTime,
    /// The rate is not finite or outside
    /// [`MIN_MUSIC_RATE`](crate::MIN_MUSIC_RATE)..=[`MAX_MUSIC_RATE`](crate::MAX_MUSIC_RATE).
    #[error("clock rate out of range")]
    InvalidRate,
}

/// Smoothed output axis: stream frame `f_ref + (t − t_ref) · slope` leaves the device at
/// engine time `t`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OutputModel {
    t_ref_ns: u64,
    /// The axis stands still from here on (no newer report).
    until_ns: u64,
    f_ref: f64,
    /// Stream frames per nanosecond.
    slope: f64,
    sample_rate: u32,
}

impl OutputModel {
    fn at(&self, t_ns: u64) -> f64 {
        self.extrapolate(t_ns.min(self.until_ns))
    }

    /// The line without the stall stop: what a report should match.
    fn extrapolate(&self, t_ns: u64) -> f64 {
        let dt = i128::from(t_ns) - i128::from(self.t_ref_ns);
        self.f_ref + dt as f64 * self.slope
    }

    fn ms_per_frame(&self) -> f64 {
        1000.0 / f64::from(self.sample_rate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mapping {
    /// The track position heard at the frame, held at `hold_ms` at least.
    Track {
        music: Option<MusicSnapshot>,
        hold_ms: f64,
    },
    /// Off the track: `time_ms` at output frame `origin`, then `speed` track ms per
    /// stream-frame ms.
    Free {
        origin: f64,
        time_ms: f64,
        speed: f64,
    },
}

/// What [`GameClock::update`] knows at one tick, by value: converts engine timestamps
/// (input events, the tick itself) to track time without touching shared state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockSnapshot {
    model: OutputModel,
    /// Output frame at the previous tick: earlier timestamps map here, so time never
    /// goes back when the model is reset backwards. Events of this tick lie after it.
    floor: f64,
    rate: f64,
    running: bool,
    mapping: Mapping,
}

impl ClockSnapshot {
    /// Stream frame leaving the device at engine time `t_ns` (smoothed), not before the
    /// frame of the previous tick.
    pub fn output_frame_at(&self, t_ns: u64) -> f64 {
        self.model.at(t_ns).max(self.floor)
    }

    /// Track time in milliseconds heard at engine time `t_ns`, without offsets.
    pub fn track_time_at(&self, t_ns: u64) -> f64 {
        self.time_at_frame(self.output_frame_at(t_ns))
    }

    fn time_at_frame(&self, frame: f64) -> f64 {
        let ms_per_frame = self.model.ms_per_frame();
        match self.mapping {
            Mapping::Track { music, hold_ms } => {
                let track = music.map_or(0.0, |m| m.track_frame_at(frame));
                (track * ms_per_frame).max(hold_ms)
            }
            Mapping::Free {
                origin,
                time_ms,
                speed,
            } => time_ms + (frame - origin).max(0.0) * ms_per_frame * speed,
        }
    }

    /// Playback rate of the clock (1 without rate mods).
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// The clock is started (the track may still be heard stopping after a pause).
    pub fn is_running(&self) -> bool {
        self.running
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Track {
        hold_ms: f64,
    },
    Free {
        origin: f64,
        time_ms: f64,
        /// Before the track: `play_at` is queued while running. After the end: `None`.
        lead_in: Option<bool>,
    },
}

/// Clock of the update thread. Call [`update`](Self::update) once per tick with the
/// latest reads of [`AudioPosition`](crate::AudioPosition) and
/// [`MusicPosition`](crate::MusicPosition); it is arithmetic only, allocation-free.
/// Bound to one output stream and the track loaded when it was created.
#[derive(Debug, Clone)]
pub struct GameClock {
    model: Option<OutputModel>,
    last_report: Option<PositionSnapshot>,
    /// Output frame at the last tick; time does not go below it.
    floor: f64,
    /// `floor` before the last tick: the lower bound for that tick's events.
    tick_floor: f64,
    rate: f64,
    running: bool,
    /// Generation of the clock's last seek; older snapshots describe the old position.
    generation: u32,
    music: Option<MusicSnapshot>,
    mode: Mode,
    /// The last seek in the track is heard: time follows its position.
    seek_heard: bool,
    resets: u64,
}

impl GameClock {
    /// Stopped at track time 0. `track` is the loaded track's player.
    pub fn new(track: &impl TrackControl) -> Self {
        Self {
            model: None,
            last_report: None,
            floor: f64::NEG_INFINITY,
            tick_floor: f64::NEG_INFINITY,
            rate: 1.0,
            running: false,
            generation: track.generation(),
            music: None,
            mode: Mode::Track {
                hold_ms: f64::NEG_INFINITY,
            },
            seek_heard: true,
            resets: 0,
        }
    }

    /// Advances to engine time `now_ns` with the latest position reads. `None` until the
    /// first output report.
    pub fn update(
        &mut self,
        now_ns: u64,
        output: Option<PositionSnapshot>,
        music: Option<MusicSnapshot>,
    ) -> Option<ClockSnapshot> {
        if let Some(report) = output {
            self.observe(now_ns, report);
        }
        if music.is_some() {
            self.music = music;
        }
        let frame = self.model?.at(now_ns).max(self.floor);
        self.tick_floor = self.floor;
        self.floor = frame;
        self.advance(frame);
        self.snapshot()
    }

    /// A seek in the track is not heard yet: until it is, time follows the position
    /// before the seek. Seeks into the lead-in move time at once.
    pub fn is_seeking(&self) -> bool {
        matches!(self.mode, Mode::Track { .. }) && !self.seek_heard
    }

    /// Times the output model was reset (a report too far off). Grows on xruns.
    pub fn resets(&self) -> u64 {
        self.resets
    }

    /// Starts the clock (and the track, or schedules it when time reaches 0).
    ///
    /// On [`ClockError::Track`] the clock reflects the commands that were queued and is
    /// still stopped; call again.
    pub fn start(&mut self, now_ns: u64, track: &mut impl TrackControl) -> Result<(), ClockError> {
        if self.running {
            return Ok(());
        }
        let frame = self.now_frame(now_ns)?;
        match self.mode {
            Mode::Track { .. } => track.play()?,
            Mode::Free {
                time_ms,
                lead_in: Some(_),
                ..
            } if time_ms >= 0.0 => {
                // Stopped past 0 before the track was heard: start it where time is.
                self.seek_track(time_ms, track)?;
                track.play()?;
            }
            Mode::Free {
                time_ms, lead_in, ..
            } => {
                if lead_in.is_some() {
                    track.play_at(self.zero_frame(frame, time_ms, self.rate))?;
                }
                self.mode = Mode::Free {
                    origin: frame,
                    time_ms,
                    lead_in: lead_in.map(|_| true),
                };
            }
        }
        self.running = true;
        Ok(())
    }

    /// Stops the clock and the track. In the track, time follows what is heard: it
    /// stops one output latency later. On [`ClockError::Track`], call again.
    pub fn stop(&mut self, now_ns: u64, track: &mut impl TrackControl) -> Result<(), ClockError> {
        if !self.running {
            return Ok(());
        }
        let frame = self.now_frame(now_ns)?;
        match self.mode {
            Mode::Track { .. } => {
                track.pause()?;
                self.running = false;
            }
            Mode::Free { lead_in, .. } => {
                // After the end the callback still counts as playing: a later seek must
                // not start the track.
                track.pause()?;
                if lead_in.is_some() {
                    // The callback may have started the track already (it writes one
                    // output latency ahead): back to 0 for the next start.
                    track.seek(0)?;
                    self.generation = track.generation();
                }
                self.halt(frame);
            }
        }
        Ok(())
    }

    /// Moves to track time `time_ms`, keeping running or stopped. Negative time is
    /// lead-in: the track waits at 0 and time runs off it at once. In the track, time
    /// moves when the seek is heard.
    ///
    /// On [`ClockError::Track`] the clock reflects the commands that were queued and
    /// may be left stopped: call `seek` again, then [`start`](Self::start) if it should
    /// run.
    pub fn seek(
        &mut self,
        now_ns: u64,
        time_ms: f64,
        track: &mut impl TrackControl,
    ) -> Result<(), ClockError> {
        if !time_ms.is_finite() {
            return Err(ClockError::InvalidTime);
        }
        let frame = self.now_frame(now_ns)?;
        let was_running = self.running;
        if time_ms < 0.0 {
            track.pause()?;
            self.halt(frame);
            track.seek(0)?;
            self.generation = track.generation();
            self.mode = Mode::Free {
                origin: frame,
                time_ms,
                lead_in: Some(false),
            };
            if was_running {
                self.start(now_ns, track)?;
            }
        } else {
            // A running track keeps playing through a seek. Off the track, stop first
            // (cancels a lead-in's queued `play_at`), then seek and play.
            let restart = was_running && !matches!(self.mode, Mode::Track { .. });
            if restart {
                track.pause()?;
                self.halt(frame);
            }
            self.seek_track(time_ms, track)?;
            if restart {
                track.play()?;
                self.running = true;
            }
        }
        Ok(())
    }

    /// Sets the rate of the clock: how fast time runs off the track, and the factor of
    /// real-time offsets. The track gets `tempo` (see [`MusicPlayer::set_rate`]); the
    /// rest of the rate is the track's frequency, which the caller sets with
    /// [`TrackControl::adjust`] (ADR-0055). The track takes it when the change is heard;
    /// on the track, time follows the music position. A queued lead-in start is
    /// rescheduled. If that fails, the track already has the new tempo: repeat the call.
    pub fn set_rate(
        &mut self,
        now_ns: u64,
        rate: f64,
        tempo: f64,
        track: &mut impl TrackControl,
    ) -> Result<(), ClockError> {
        let range = crate::MIN_MUSIC_RATE..=crate::MAX_MUSIC_RATE;
        if !range.contains(&rate) || !range.contains(&tempo) {
            return Err(ClockError::InvalidRate);
        }
        let frame = self.now_frame(now_ns)?;
        track.set_rate(tempo)?;
        if let Mode::Free { lead_in, .. } = self.mode {
            let time_ms = self.free_time(frame);
            if lead_in == Some(true) {
                // Replaces the pending start; if it already happened, the hand-over
                // holds time until the track catches up.
                track.play_at(self.zero_frame(frame, time_ms, rate))?;
            }
            self.mode = Mode::Free {
                origin: frame,
                time_ms,
                lead_in,
            };
        }
        self.rate = rate;
        Ok(())
    }

    /// Seeks the track to `time_ms` (≥ 0) and follows it.
    fn seek_track(
        &mut self,
        time_ms: f64,
        track: &mut impl TrackControl,
    ) -> Result<(), ClockError> {
        let ms_per_frame = self.model.map_or(1.0, |m| m.ms_per_frame());
        track.seek((time_ms / ms_per_frame).round() as u64)?;
        self.generation = track.generation();
        self.mode = Mode::Track {
            hold_ms: f64::NEG_INFINITY,
        };
        self.seek_heard = false;
        Ok(())
    }

    /// Stops the clock; free time freezes at output frame `frame`.
    fn halt(&mut self, frame: f64) {
        if let Mode::Free { lead_in, .. } = self.mode {
            self.mode = Mode::Free {
                origin: frame,
                time_ms: self.free_time(frame),
                lead_in: lead_in.map(|_| false),
            };
        }
        self.running = false;
    }

    fn now_frame(&self, now_ns: u64) -> Result<f64, ClockError> {
        let model = self.model.ok_or(ClockError::NoOutput)?;
        Ok(model.at(now_ns).max(self.floor))
    }

    /// Output frame where free time, running from `time_ms` at `frame` at `rate`,
    /// reaches 0; off by up to half a frame, which the hand-over absorbs. Before the
    /// first frame is heard (`frame` < 0) it may round to 0: the track starts at once.
    fn zero_frame(&self, frame: f64, time_ms: f64, rate: f64) -> u64 {
        let ms_per_frame = self.model.map_or(1.0, |m| m.ms_per_frame());
        (frame - time_ms / (ms_per_frame * rate)).round() as u64
    }

    fn free_time(&self, frame: f64) -> f64 {
        self.snapshot().map_or(0.0, |s| s.time_at_frame(frame))
    }

    fn observe(&mut self, now_ns: u64, report: PositionSnapshot) {
        if self.last_report == Some(report) || report.sample_rate == 0 {
            return;
        }
        self.last_report = Some(report);
        let out_ns = report.timestamp_ns.saturating_add(report.latency_ns);
        let nominal = f64::from(report.sample_rate) / 1e9;
        let until_ns = report.timestamp_ns.saturating_add(STALE_NS);
        if let Some(m) = self.model {
            if m.sample_rate == report.sample_rate {
                let error_ns = (report.frame as f64 - m.extrapolate(out_ns)) / nominal;
                if error_ns.abs() <= RESET_ERROR_NS {
                    let correction = (error_ns / CORRECTION_NS).clamp(-MAX_SLEW, MAX_SLEW);
                    // Pivot at the tick: the axis stays continuous.
                    self.model = Some(OutputModel {
                        t_ref_ns: now_ns,
                        until_ns,
                        // The report confirmed the line itself, also past a stall stop.
                        f_ref: m.extrapolate(now_ns),
                        slope: nominal * (1.0 + correction),
                        sample_rate: m.sample_rate,
                    });
                    return;
                }
            } else {
                // Another frame axis: the old floor means nothing on it.
                self.floor = f64::NEG_INFINITY;
                self.tick_floor = f64::NEG_INFINITY;
            }
            self.resets += 1;
        }
        self.model = Some(OutputModel {
            t_ref_ns: out_ns,
            until_ns,
            f_ref: report.frame as f64,
            slope: nominal,
            sample_rate: report.sample_rate,
        });
    }

    /// Switches between the track and free time at this tick's output frame.
    fn advance(&mut self, frame: f64) {
        let Some(music) = self.music else { return };
        let Some(model) = self.model else { return };
        if music.generation != self.generation {
            return;
        }
        let anchor = music.anchor;
        let heard = frame >= anchor.device_frame as f64;
        // Anchors of a generation start with its seek: any of them heard means the seek is.
        self.seek_heard |= heard;
        match self.mode {
            Mode::Track { .. } if music.ended && anchor.speed == 0.0 && heard => {
                self.mode = Mode::Free {
                    origin: anchor.device_frame as f64,
                    time_ms: anchor.track_frame * model.ms_per_frame(),
                    lead_in: None,
                };
            }
            Mode::Free {
                lead_in: Some(true),
                ..
            } if anchor.moves() && heard => {
                let free = self.free_time(frame);
                let track = music.track_frame_at(frame) * model.ms_per_frame();
                // A late start: hold until the track catches up instead of going back.
                let hold_ms = if track < free {
                    free
                } else {
                    f64::NEG_INFINITY
                };
                self.mode = Mode::Track { hold_ms };
            }
            _ => {}
        }
    }

    fn snapshot(&self) -> Option<ClockSnapshot> {
        let model = self.model?;
        let mapping = match self.mode {
            Mode::Track { hold_ms } => Mapping::Track {
                music: self.music,
                hold_ms,
            },
            Mode::Free {
                origin, time_ms, ..
            } => Mapping::Free {
                origin,
                time_ms,
                speed: if self.running { self.rate } else { 0.0 },
            },
        };
        Some(ClockSnapshot {
            model,
            floor: self.tick_floor,
            rate: self.rate,
            running: self.running,
            mapping,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::MusicAnchor;

    const RATE: u32 = 48_000;
    const PERIOD: u64 = 128;
    const LATENCY_NS: u64 = 10_000_000;
    const MS: u64 = 1_000_000;
    /// Half a millisecond in stream frames.
    const HALF_MS: f64 = RATE as f64 / 2000.0;

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Command {
        Play,
        PlayAt(u64),
        Pause,
        Seek(u64),
        Rate(f64),
    }

    #[derive(Default)]
    struct FakeTrack {
        commands: Vec<Command>,
        generation: u32,
        /// Refuse the next command of this kind once.
        refuse: Option<Command>,
    }

    impl FakeTrack {
        fn send(&mut self, command: Command) -> Result<(), PlayError> {
            if self
                .refuse
                .is_some_and(|r| std::mem::discriminant(&r) == std::mem::discriminant(&command))
            {
                self.refuse = None;
                return Err(PlayError::QueueFull);
            }
            self.commands.push(command);
            Ok(())
        }
    }

    impl TrackControl for FakeTrack {
        fn play(&mut self) -> Result<(), PlayError> {
            self.send(Command::Play)
        }

        fn play_at(&mut self, device_frame: u64) -> Result<(), PlayError> {
            self.send(Command::PlayAt(device_frame))
        }

        fn pause(&mut self) -> Result<(), PlayError> {
            self.send(Command::Pause)
        }

        fn seek(&mut self, frame: u64) -> Result<(), PlayError> {
            self.send(Command::Seek(frame))?;
            self.generation += 1;
            Ok(())
        }

        fn set_rate(&mut self, rate: f64) -> Result<(), PlayError> {
            self.send(Command::Rate(rate))
        }

        fn adjust(&mut self, _: &[TrackAdjustment]) -> Result<(), PlayError> {
            Ok(())
        }

        fn generation(&self) -> u32 {
            self.generation
        }
    }

    /// Deterministic noise in −1..1.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        }
    }

    /// An output stream: buffers of `PERIOD` frames, a clock off by `drift`, reports
    /// whose timestamp and latency jitter by up to `jitter_ns` each.
    struct Device {
        start_ns: u64,
        drift: f64,
        jitter_ns: f64,
        noise: Lcg,
        /// Added to the true frame axis (xruns).
        shift: i64,
        latency_ns: u64,
        next_buffer: u64,
        report: Option<PositionSnapshot>,
    }

    impl Device {
        fn new(drift: f64, jitter_ns: f64) -> Self {
            Self {
                start_ns: 1_000 * MS,
                drift,
                jitter_ns,
                noise: Lcg(7),
                shift: 0,
                latency_ns: LATENCY_NS,
                next_buffer: 0,
                report: None,
            }
        }

        /// True stream frame leaving the device at `t_ns`.
        fn frame_at(&self, t_ns: u64) -> f64 {
            let dt = t_ns as f64 - self.start_ns as f64;
            dt * f64::from(RATE) * (1.0 + self.drift) / 1e9 + self.shift as f64
        }

        /// When stream frame `frame` leaves the device.
        fn out_ns(&self, frame: u64) -> u64 {
            let f = frame as f64 - self.shift as f64;
            self.start_ns + (f * 1e9 / (f64::from(RATE) * (1.0 + self.drift))) as u64
        }

        /// Latest report at `now_ns`: buffers are written one latency before they leave.
        fn poll(&mut self, now_ns: u64) -> Option<PositionSnapshot> {
            loop {
                let frame = self.next_buffer * PERIOD;
                let out = self.out_ns(frame);
                if out < self.latency_ns || out - self.latency_ns > now_ns {
                    break;
                }
                let ts = out - self.latency_ns;
                let ts_noise = (self.noise.next() * self.jitter_ns) as i64;
                let lat_noise = (self.noise.next() * self.jitter_ns) as i64;
                self.report = Some(PositionSnapshot {
                    frame,
                    timestamp_ns: ts.saturating_add_signed(ts_noise),
                    latency_ns: self.latency_ns.saturating_add_signed(lat_noise),
                    sample_rate: RATE,
                });
                self.next_buffer += 1;
            }
            self.report
        }
    }

    fn playing_from(device_frame: u64, track_frame: u64, speed: f64) -> MusicSnapshot {
        let anchor = MusicAnchor {
            device_frame,
            track_frame: track_frame as f64,
            speed,
            accel: 0.0,
        };
        MusicSnapshot {
            anchor,
            previous: anchor,
            generation: 0,
            ended: false,
        }
    }

    fn change(
        music: MusicSnapshot,
        device_frame: u64,
        track_frame: u64,
        speed: f64,
    ) -> MusicSnapshot {
        MusicSnapshot {
            anchor: MusicAnchor {
                device_frame,
                track_frame: track_frame as f64,
                speed,
                accel: 0.0,
            },
            previous: music.anchor,
            ..music
        }
    }

    fn ms(frames: f64) -> f64 {
        frames * 1000.0 / f64::from(RATE)
    }

    #[test]
    fn nothing_before_the_first_report() {
        let mut track = FakeTrack::default();
        let mut clock = GameClock::new(&track);
        assert_eq!(clock.update(5 * MS, None, None), None);
        assert_eq!(clock.start(5 * MS, &mut track), Err(ClockError::NoOutput));
        assert!(track.commands.is_empty());
    }

    #[test]
    fn follows_a_jittery_drifting_device_closely_and_monotonically() {
        for drift in [-100e-6, 0.0, 100e-6] {
            let mut device = Device::new(drift, 1.5e6);
            let track = FakeTrack::default();
            let mut clock = GameClock::new(&track);
            let music = playing_from(0, 0, 1.0);
            let mut last = f64::NEG_INFINITY;
            let mut worst: f64 = 0.0;
            for tick in 0..20_000u64 {
                let now = device.start_ns + tick * MS;
                let Some(s) = clock.update(now, device.poll(now), Some(music)) else {
                    continue;
                };
                let frame = s.output_frame_at(now);
                assert!(frame >= last, "drift {drift}: went back at tick {tick}");
                last = frame;
                if tick > 1_000 {
                    worst = worst.max((frame - device.frame_at(now)).abs());
                }
                assert!((s.track_time_at(now) - ms(frame)).abs() < 1e-9);
            }
            assert!(worst < HALF_MS, "drift {drift}: error {} ms", ms(worst));
            assert_eq!(clock.resets(), 0, "drift {drift}");
        }
    }

    #[test]
    fn an_xrun_resets_forward_and_a_backward_reset_holds_time() {
        let mut device = Device::new(0.0, 0.0);
        let track = FakeTrack::default();
        let mut clock = GameClock::new(&track);
        let music = playing_from(0, 0, 1.0);
        let run = |clock: &mut GameClock, device: &mut Device, from: u64, to: u64| {
            let mut last = f64::NEG_INFINITY;
            for tick in from..to {
                let now = device.start_ns + tick * MS;
                let s = clock.update(now, device.poll(now), Some(music)).unwrap();
                let t = s.track_time_at(now);
                assert!(t >= last, "went back at tick {tick}");
                last = t;
            }
            last
        };
        run(&mut clock, &mut device, 20, 1_000);
        // The device skips 100 ms of frames.
        device.shift += i64::from(RATE) / 10;
        device.next_buffer += u64::from(RATE) / 10 / PERIOD;
        let t = run(&mut clock, &mut device, 1_000, 1_100);
        assert_eq!(clock.resets(), 1);
        let now = device.start_ns + 1_099 * MS;
        assert!((t - ms(device.frame_at(now))).abs() < 0.5);

        // The axis jumps 20 ms back: time stands until it catches up.
        device.shift -= i64::from(RATE) / 50;
        device.next_buffer -= u64::from(RATE) / 50 / PERIOD;
        run(&mut clock, &mut device, 1_100, 1_200);
        assert_eq!(clock.resets(), 2);
        let now = device.start_ns + 1_199 * MS;
        let s = clock.update(now, device.poll(now), Some(music)).unwrap();
        assert!((s.output_frame_at(now) - device.frame_at(now)).abs() < 1.0);
    }

    /// A clock on an ideal device after `ticks` ms, with the track as `music`.
    fn settled(ticks: u64, music: MusicSnapshot) -> (Device, GameClock, FakeTrack) {
        let mut device = Device::new(0.0, 0.0);
        let track = FakeTrack::default();
        let mut clock = GameClock::new(&track);
        for tick in 0..ticks {
            let now = device.start_ns + tick * MS;
            clock.update(now, device.poll(now), Some(music));
        }
        (device, clock, track)
    }

    fn tick(
        clock: &mut GameClock,
        device: &mut Device,
        ms_: u64,
        music: MusicSnapshot,
    ) -> ClockSnapshot {
        let now = device.start_ns + ms_ * MS;
        clock.update(now, device.poll(now), Some(music)).unwrap()
    }

    fn time(clock: &mut GameClock, device: &mut Device, ms_: u64, music: MusicSnapshot) -> f64 {
        let now = device.start_ns + ms_ * MS;
        tick(clock, device, ms_, music).track_time_at(now)
    }

    #[test]
    fn pause_and_seek_show_when_they_are_heard() {
        let music = playing_from(0, 0, 1.0);
        let (mut device, mut clock, _) = settled(1_000, music);
        // Paused at frame 48 480, written at 1000 ms and heard one latency later.
        let paused = change(music, 48_480, 48_480, 0.0);
        assert!((time(&mut clock, &mut device, 1_005, paused) - 1_005.0).abs() < 0.01);
        assert!((time(&mut clock, &mut device, 1_020, paused) - 1_010.0).abs() < 0.01);
        // Seek back to 200 ms while playing, at frame 52 800 (heard at 1100 ms).
        let seeked = change(paused, 52_800, 9_600, 1.0);
        assert!((time(&mut clock, &mut device, 1_095, seeked) - 1_010.0).abs() < 0.01);
        assert!((time(&mut clock, &mut device, 1_105, seeked) - 205.0).abs() < 0.01);
    }

    #[test]
    fn a_seek_in_the_track_is_pending_until_heard() {
        let music = playing_from(0, 0, 1.0);
        let (mut device, mut clock, mut track) = settled(1_000, music);
        assert!(!clock.is_seeking());
        clock
            .seek(device.start_ns + 1_000 * MS, 200.0, &mut track)
            .unwrap();
        assert!(clock.is_seeking());
        // The callback has not taken the seek yet.
        tick(&mut clock, &mut device, 1_001, music);
        assert!(clock.is_seeking());
        // Taken at frame 48 576, heard at 1012 ms; time is the old position until then.
        let seeked = MusicSnapshot {
            generation: 1,
            ..change(music, 48_576, 9_600, 1.0)
        };
        assert!((time(&mut clock, &mut device, 1_005, seeked) - 1_005.0).abs() < 0.01);
        assert!(clock.is_seeking());
        assert!((time(&mut clock, &mut device, 1_013, seeked) - 201.0).abs() < 0.01);
        assert!(!clock.is_seeking());
        // Pausing and playing are not seeks.
        let paused = change(seeked, 49_000, 9_904, 0.0);
        tick(&mut clock, &mut device, 1_014, paused);
        assert!(!clock.is_seeking());
    }

    #[test]
    fn a_seek_into_the_lead_in_is_not_pending() {
        let music = playing_from(0, 0, 1.0);
        let (device, mut clock, mut track) = settled(1_000, music);
        clock
            .seek(device.start_ns + 1_000 * MS, -500.0, &mut track)
            .unwrap();
        assert!(!clock.is_seeking());
    }

    #[test]
    fn track_speed_comes_from_the_anchor() {
        let music = playing_from(0, 0, 1.0);
        let (mut device, mut clock, _) = settled(1_000, music);
        let fast = change(music, 48_000, 48_000, 1.5);
        let a = time(&mut clock, &mut device, 1_100, fast);
        let b = time(&mut clock, &mut device, 1_200, fast);
        assert!((b - a - 150.0).abs() < 0.01, "{a} {b}");
    }

    #[test]
    fn a_moving_frequency_slows_time_along_the_anchor_curve() {
        let music = playing_from(0, 0, 1.0);
        let (mut device, mut clock, _) = settled(1_000, music);
        // From 1 s on the frequency falls from 1 to 0 over 1 s (48000 frames).
        let mut slowing = change(music, 48_000, 48_000, 1.0);
        slowing.anchor.accel = -1.0 / 48_000.0;
        let a = time(&mut clock, &mut device, 1_250, slowing);
        let b = time(&mut clock, &mut device, 1_750, slowing);
        // Track time at real t past 1 s: t − t²/2 (in seconds).
        let curve = |t: f64| 1_000.0 + (t - t * t / 2.0) * 1_000.0;
        assert!((a - curve(0.25)).abs() < 0.01, "{a}");
        assert!((b - curve(0.75)).abs() < 0.01, "{b}");
    }

    #[test]
    fn input_between_ticks_maps_between_their_times() {
        let music = playing_from(0, 0, 1.0);
        let (mut device, mut clock, _) = settled(1_000, music);
        let s = tick(&mut clock, &mut device, 1_000, music);
        let at = |t_ms: f64| s.track_time_at(device.start_ns + (t_ms * 1e6) as u64);
        assert!((at(999.25) - 999.25).abs() < 0.01);
        assert!((at(999.75) - 999.75).abs() < 0.01);
        // Earlier than the tick floor: held at the tick before.
        let s2 = tick(&mut clock, &mut device, 1_001, music);
        let at2 = s2.track_time_at(device.start_ns + 999 * MS);
        assert!((at2 - 1_000.0).abs() < 0.01, "{at2}");
    }

    #[test]
    fn lead_in_runs_off_the_track_and_hands_over_on_the_scheduled_frame() {
        let idle = playing_from(0, 0, 0.0);
        let (mut device, mut clock, mut track) = settled(100, idle);
        let now = device.start_ns + 100 * MS;
        clock.seek(now, -500.0, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        // Frame 4800 is heard at 100 ms; time 0 is 500 ms later.
        let zero = 4_800 + 24_000;
        assert_eq!(
            track.commands,
            [Command::Pause, Command::Seek(0), Command::PlayAt(zero)]
        );
        let music = MusicSnapshot {
            generation: 1,
            ..idle
        };
        let mut last = f64::NEG_INFINITY;
        for t in 101..1_000 {
            // The callback starts the track on the frame, written one latency earlier.
            let music = if t >= 590 {
                change(music, zero, 0, 1.0)
            } else {
                music
            };
            let now_t = time(&mut clock, &mut device, t, music);
            let want = t as f64 - 600.0;
            assert!((now_t - want).abs() < 0.03, "{t}: {now_t} vs {want}");
            assert!(now_t >= last);
            last = now_t;
        }
        assert!(matches!(clock.mode, Mode::Track { hold_ms } if hold_ms == f64::NEG_INFINITY));
    }

    #[test]
    fn a_late_track_start_holds_time_instead_of_going_back() {
        let idle = playing_from(0, 0, 0.0);
        let (mut device, mut clock, mut track) = settled(100, idle);
        let now = device.start_ns + 100 * MS;
        clock.seek(now, -100.0, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        let music = MusicSnapshot {
            generation: 1,
            ..idle
        };
        // Due at 200 ms (heard), started 10 ms (480 frames) late.
        let late = change(music, 4_800 + 4_800 + 480, 0, 1.0);
        let mut last = f64::NEG_INFINITY;
        for t in 101..400 {
            let m = if t >= 200 { late } else { music };
            let now_t = time(&mut clock, &mut device, t, m);
            assert!(now_t >= last, "{t}");
            last = now_t;
            if t > 230 {
                assert!((now_t - (t as f64 - 210.0)).abs() < 0.03, "{t}: {now_t}");
            }
        }
    }

    #[test]
    fn time_runs_on_after_the_end() {
        let music = playing_from(0, 0, 1.0);
        let (mut device, mut clock, mut track) = settled(1_000, music);
        clock
            .start(device.start_ns + 1_000 * MS, &mut track)
            .unwrap();
        let ended = MusicSnapshot {
            ended: true,
            ..change(music, 48_000, 48_000, 0.0)
        };
        let mut last = f64::NEG_INFINITY;
        for t in 1_000..1_100 {
            let now_t = time(&mut clock, &mut device, t, ended);
            assert!((now_t - t as f64).abs() < 0.01, "{t}: {now_t}");
            assert!(now_t >= last);
            last = now_t;
        }
    }

    #[test]
    fn a_stale_end_before_a_seek_is_heard_does_not_free_the_clock() {
        let music = playing_from(0, 0, 1.0);
        let (mut device, mut clock, mut track) = settled(1_000, music);
        let now = device.start_ns + 1_000 * MS;
        clock.seek(now, 300.0, &mut track).unwrap();
        assert_eq!(track.commands, [Command::Seek(14_400)]);
        let stale = MusicSnapshot {
            ended: true,
            ..change(music, 48_000, 48_000, 0.0)
        };
        tick(&mut clock, &mut device, 1_050, stale);
        assert!(matches!(clock.mode, Mode::Track { .. }));
    }

    #[test]
    fn stopping_in_lead_in_freezes_time_and_cancels_the_start() {
        let idle = playing_from(0, 0, 0.0);
        let (mut device, mut clock, mut track) = settled(100, idle);
        let music = MusicSnapshot {
            generation: 1,
            ..idle
        };
        let now = device.start_ns + 100 * MS;
        clock.seek(now, -1_000.0, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        time(&mut clock, &mut device, 300, music);
        clock.stop(device.start_ns + 300 * MS, &mut track).unwrap();
        assert_eq!(track.commands[3..], [Command::Pause, Command::Seek(0)]);
        let music = MusicSnapshot {
            generation: 2,
            ..idle
        };
        let frozen = time(&mut clock, &mut device, 500, music);
        assert!((frozen + 800.0).abs() < 0.01, "{frozen}");
        // Restarting schedules the track 800 ms on.
        clock.start(device.start_ns + 500 * MS, &mut track).unwrap();
        assert_eq!(
            track.commands.last(),
            Some(&Command::PlayAt(24_000 + 38_400))
        );
    }

    #[test]
    fn rate_scales_free_time() {
        let idle = playing_from(0, 0, 0.0);
        let (mut device, mut clock, mut track) = settled(100, idle);
        let music = MusicSnapshot {
            generation: 1,
            ..idle
        };
        let now = device.start_ns + 100 * MS;
        clock.seek(now, -3_000.0, &mut track).unwrap();
        clock.set_rate(now, 1.5, 1.5, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        assert_eq!(
            track.commands.last(),
            Some(&Command::PlayAt(4_800 + 96_000))
        );
        let t = time(&mut clock, &mut device, 300, music);
        assert!((t + 2_700.0).abs() < 0.01, "{t}");
        assert_eq!(tick(&mut clock, &mut device, 301, music).rate(), 1.5);
    }

    #[test]
    fn sparse_ticks_and_a_latency_step_stay_close_without_a_reset() {
        let mut device = Device::new(50e-6, 1e6);
        let track = FakeTrack::default();
        let mut clock = GameClock::new(&track);
        let music = playing_from(0, 0, 1.0);
        let mut last = f64::NEG_INFINITY;
        let base = device.start_ns;
        for tick in 0..400u64 {
            let now = base + tick * 25 * MS;
            if tick == 200 {
                // The output latency grows by 3 ms: frames leave 3 ms later.
                device.start_ns += 3 * MS;
            }
            let Some(s) = clock.update(now, device.poll(now), Some(music)) else {
                continue;
            };
            let frame = s.output_frame_at(now);
            assert!(frame >= last, "went back at tick {tick}");
            last = frame;
            if (40..200).contains(&tick) || tick > 260 {
                // One report seen per 25 ms tick averages jitter less than at 1 kHz.
                let err = ms(frame - device.frame_at(now)).abs();
                assert!(err < 1.0, "tick {tick}: {err} ms");
            }
        }
        assert_eq!(clock.resets(), 0);
    }

    #[test]
    fn a_stalled_device_stops_time_and_resumes_without_going_back() {
        let mut device = Device::new(0.0, 0.0);
        let track = FakeTrack::default();
        let mut clock = GameClock::new(&track);
        let music = playing_from(0, 0, 1.0);
        let mut last = f64::NEG_INFINITY;
        let base = device.start_ns;
        for t in 0..3_000u64 {
            let now = base + t * MS;
            // No reports between 1 s and 2 s; then the device goes on from where it was.
            let report = if (1_000..2_000).contains(&t) {
                None
            } else {
                if t == 2_000 {
                    device.start_ns += 1_000 * MS;
                }
                device.poll(now)
            };
            let s = clock.update(now, report, Some(music));
            let Some(s) = s else { continue };
            let time = s.track_time_at(now);
            assert!(time >= last, "{t}");
            last = time;
            if t == 1_500 {
                // Stopped 250 ms after the last report's timestamp (just before 1 s).
                assert!((1_247.0..=1_250.0).contains(&time), "{time}");
            }
        }
        // 1 s behind the engine clock now, and running again.
        assert!((last - 1_999.0).abs() < 0.5, "{last}");
    }

    #[test]
    fn stopping_in_lead_in_after_the_callback_started_the_track_rewinds_it() {
        let idle = playing_from(0, 0, 0.0);
        let (mut device, mut clock, mut track) = settled(100, idle);
        let now = device.start_ns + 100 * MS;
        clock.seek(now, -500.0, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        let zero = 4_800 + 24_000;
        // Written one latency ahead: the track runs in the callback, not heard yet.
        let started = change(
            MusicSnapshot {
                generation: 1,
                ..idle
            },
            zero,
            0,
            1.0,
        );
        time(&mut clock, &mut device, 595, started);
        clock.stop(device.start_ns + 595 * MS, &mut track).unwrap();
        assert_eq!(track.commands[3..], [Command::Pause, Command::Seek(0)]);
        clock.start(device.start_ns + 700 * MS, &mut track).unwrap();
        // 5 ms of lead-in left.
        assert_eq!(track.commands.last(), Some(&Command::PlayAt(33_600 + 240)));
    }

    #[test]
    fn a_refused_command_leaves_the_generation_in_step() {
        let idle = playing_from(0, 0, 0.0);
        let (_, mut clock, mut track) = settled(100, idle);
        let now = 1_100 * MS;
        clock.start(now, &mut track).unwrap();
        track.refuse = Some(Command::PlayAt(0));
        let err = clock.seek(now, -500.0, &mut track);
        assert_eq!(err, Err(ClockError::Track(PlayError::QueueFull)));
        assert_eq!(clock.generation, track.generation());
        assert!(!clock.running);
        clock.start(now, &mut track).unwrap();
        assert!(matches!(track.commands.last(), Some(Command::PlayAt(_))));
        assert!(matches!(
            clock.mode,
            Mode::Free {
                lead_in: Some(true),
                ..
            }
        ));
    }

    #[test]
    fn set_rate_checks_the_rate_and_reschedules_the_lead_in_start() {
        let idle = playing_from(0, 0, 0.0);
        let (_, mut clock, mut track) = settled(100, idle);
        let now = 1_100 * MS;
        for bad in [0.0, -1.0, 0.04, 4.5, f64::NAN, f64::INFINITY] {
            assert_eq!(
                clock.set_rate(now, bad, bad, &mut track),
                Err(ClockError::InvalidRate)
            );
            assert_eq!(
                clock.set_rate(now, 1.0, bad, &mut track),
                Err(ClockError::InvalidRate)
            );
        }
        assert!(track.commands.is_empty());
        clock.seek(now, -1_000.0, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        assert_eq!(
            track.commands.last(),
            Some(&Command::PlayAt(4_800 + 48_000))
        );
        clock.set_rate(now, 2.0, 2.0, &mut track).unwrap();
        let n = track.commands.len();
        assert_eq!(
            track.commands[n - 2..],
            [Command::Rate(2.0), Command::PlayAt(4_800 + 24_000)]
        );
    }

    #[test]
    fn the_track_tempo_is_set_apart_from_the_clock_rate() {
        // Nightcore: the frequency makes up the rate, the tempo stays 1.
        let idle = playing_from(0, 0, 0.0);
        let (_, mut clock, mut track) = settled(100, idle);
        let now = 1_100 * MS;
        clock.seek(now, -1_000.0, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        clock.set_rate(now, 1.5, 1.0, &mut track).unwrap();
        let n = track.commands.len();
        // The lead-in runs at the clock rate: 1000 ms of game time in 32 000 frames.
        assert_eq!(
            track.commands[n - 2..],
            [Command::Rate(1.0), Command::PlayAt(4_800 + 32_000)]
        );
    }

    #[test]
    fn seeking_out_of_lead_in_plays_the_track_and_track_stop_start_pause_and_play() {
        let idle = playing_from(0, 0, 0.0);
        let (_, mut clock, mut track) = settled(100, idle);
        let now = 1_100 * MS;
        clock.seek(now, -1_000.0, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        clock.seek(now, 1_000.0, &mut track).unwrap();
        assert_eq!(
            track.commands[3..],
            [Command::Pause, Command::Seek(48_000), Command::Play]
        );
        assert!(matches!(clock.mode, Mode::Track { .. }));
        // Running in the track: a seek needs no play.
        clock.seek(now, 2_000.0, &mut track).unwrap();
        assert_eq!(track.commands.last(), Some(&Command::Seek(96_000)));
        clock.stop(now, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        assert_eq!(track.commands[7..], [Command::Pause, Command::Play]);
    }

    #[test]
    fn a_long_output_latency_smooths_without_resets() {
        let mut device = Device::new(30e-6, 1e6);
        device.latency_ns = 300 * MS;
        let track = FakeTrack::default();
        let mut clock = GameClock::new(&track);
        let music = playing_from(0, 0, 1.0);
        let mut last = f64::NEG_INFINITY;
        for tick in 0..5_000u64 {
            let now = device.start_ns + tick * MS;
            let Some(s) = clock.update(now, device.poll(now), Some(music)) else {
                continue;
            };
            let frame = s.output_frame_at(now);
            assert!(frame >= last);
            last = frame;
            if tick > 1_500 {
                let err = ms(frame - device.frame_at(now)).abs();
                assert!(err < 0.5, "tick {tick}: {err} ms");
            }
        }
        assert_eq!(clock.resets(), 0);
    }

    #[test]
    fn a_refused_rewind_on_stop_is_retried() {
        let idle = playing_from(0, 0, 0.0);
        let (_, mut clock, mut track) = settled(100, idle);
        let now = 1_100 * MS;
        clock.seek(now, -500.0, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        track.refuse = Some(Command::Seek(0));
        assert!(clock.stop(now, &mut track).is_err());
        assert!(clock.running);
        clock.stop(now, &mut track).unwrap();
        assert_eq!(
            track.commands[3..],
            [Command::Pause, Command::Pause, Command::Seek(0)]
        );
        assert_eq!(clock.generation, track.generation());
        assert!(!clock.running);
    }

    #[test]
    fn a_seek_after_the_end_while_stopped_keeps_the_track_paused() {
        let music = playing_from(0, 0, 1.0);
        let (mut device, mut clock, mut track) = settled(1_000, music);
        clock
            .start(device.start_ns + 1_000 * MS, &mut track)
            .unwrap();
        let ended = MusicSnapshot {
            ended: true,
            ..change(music, 48_000, 48_000, 0.0)
        };
        time(&mut clock, &mut device, 1_050, ended);
        clock
            .stop(device.start_ns + 1_050 * MS, &mut track)
            .unwrap();
        clock
            .seek(device.start_ns + 1_050 * MS, 0.0, &mut track)
            .unwrap();
        assert_eq!(
            track.commands,
            [Command::Play, Command::Pause, Command::Seek(0)]
        );
        assert!(!clock.running);
    }

    #[test]
    fn a_refused_play_when_seeking_out_of_lead_in_leaves_everything_stopped() {
        let idle = playing_from(0, 0, 0.0);
        let (_, mut clock, mut track) = settled(100, idle);
        let now = 1_100 * MS;
        clock.seek(now, -500.0, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        track.refuse = Some(Command::Play);
        assert!(clock.seek(now, 1_000.0, &mut track).is_err());
        // The pause cancelled the queued start; the clock is stopped in the track.
        assert_eq!(track.commands[3..], [Command::Pause, Command::Seek(48_000)]);
        assert!(!clock.running);
        assert!(matches!(clock.mode, Mode::Track { .. }));
        clock.start(now, &mut track).unwrap();
        assert_eq!(track.commands.last(), Some(&Command::Play));
    }

    #[test]
    fn a_non_finite_seek_is_refused() {
        let idle = playing_from(0, 0, 0.0);
        let (_, mut clock, mut track) = settled(100, idle);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                clock.seek(1_100 * MS, bad, &mut track),
                Err(ClockError::InvalidTime)
            );
        }
        assert!(track.commands.is_empty());
    }

    #[test]
    fn a_stalled_update_thread_does_not_reset_the_model() {
        let mut device = Device::new(0.0, 0.0);
        let track = FakeTrack::default();
        let mut clock = GameClock::new(&track);
        let music = playing_from(0, 0, 1.0);
        let mut last = f64::NEG_INFINITY;
        for t in (0..1_000u64).chain(1_400..2_000) {
            let now = device.start_ns + t * MS;
            let Some(s) = clock.update(now, device.poll(now), Some(music)) else {
                continue;
            };
            let time = s.track_time_at(now);
            assert!(time >= last, "{t}");
            last = time;
            if t >= 1_400 {
                assert!((time - t as f64).abs() < 0.5, "{t}: {time}");
            }
        }
        assert_eq!(clock.resets(), 0);
    }

    #[test]
    fn a_refused_pause_after_the_end_is_retried() {
        let music = playing_from(0, 0, 1.0);
        let (mut device, mut clock, mut track) = settled(1_000, music);
        clock
            .start(device.start_ns + 1_000 * MS, &mut track)
            .unwrap();
        let ended = MusicSnapshot {
            ended: true,
            ..change(music, 48_000, 48_000, 0.0)
        };
        time(&mut clock, &mut device, 1_050, ended);
        let now = device.start_ns + 1_050 * MS;
        track.refuse = Some(Command::Pause);
        assert!(clock.stop(now, &mut track).is_err());
        assert!(clock.running);
        clock.stop(now, &mut track).unwrap();
        assert!(!clock.running);
        assert_eq!(track.commands, [Command::Play, Command::Pause]);
        let frozen = time(&mut clock, &mut device, 1_100, ended);
        assert!((frozen - 1_050.0).abs() < 0.01, "{frozen}");
    }

    #[test]
    fn a_refused_seek_out_of_lead_in_leaves_time_frozen_off_the_track() {
        let idle = playing_from(0, 0, 0.0);
        let (mut device, mut clock, mut track) = settled(100, idle);
        let music = MusicSnapshot {
            generation: 1,
            ..idle
        };
        let now = device.start_ns + 100 * MS;
        clock.seek(now, -500.0, &mut track).unwrap();
        clock.start(now, &mut track).unwrap();
        time(&mut clock, &mut device, 200, music);
        track.refuse = Some(Command::Seek(0));
        let at = device.start_ns + 200 * MS;
        assert!(clock.seek(at, 1_000.0, &mut track).is_err());
        assert!(!clock.running);
        assert!(matches!(
            clock.mode,
            Mode::Free {
                lead_in: Some(false),
                ..
            }
        ));
        assert_eq!(track.commands.last(), Some(&Command::Pause));
        let frozen = time(&mut clock, &mut device, 300, music);
        assert!((frozen + 400.0).abs() < 0.01, "{frozen}");
    }
}
