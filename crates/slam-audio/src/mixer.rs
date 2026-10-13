//! Hitsound mixer: plays pre-decoded samples on the audio thread, driven by commands
//! from the update thread through a lock-free queue (ARCHITECTURE §4, §6).
//!
//! The update thread owns a [`MixerHandle`], the audio stream owns the [`Mixer`]. Both
//! sides are wait-free: commands go through an SPSC ring, and samples the mixer is done
//! with go back through a second ring, so the audio thread never drops the last
//! reference to a sample (that would free memory there).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::hal::{AudioCallback, AudioError, CallbackInfo};
use crate::music::MusicSource;
use crate::sample::{MAX_CHANNELS, Sample};

/// Copies of one sample that sound at once in lazer: further plays replace the
/// longest-playing copy.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/OsuGameBase.cs (SAMPLE_CONCURRENCY),
// with osu-framework 2026.921.1: osu.Framework/Audio/Sample/SampleBassFactory.cs
// (BASS_SampleLoad max = concurrency, SampleOverrideLongestPlaying).
pub const SAMPLE_CONCURRENCY: usize = 6;

/// Highest playback rate of a voice ([`VoiceParams::rate`]).
pub const MAX_VOICE_RATE: f64 = 8.0;

/// One frame in the 32.32 fixed-point voice cursor (ADR-0039).
const FRAME: u64 = 1 << 32;
const FRACTION: u64 = FRAME - 1;

/// Mixer parameters. Rate and channels must match the output stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixerConfig {
    /// Frames per second of the output stream and of every played sample.
    pub sample_rate: u32,
    /// Interleaved channels of the output stream and of every played sample.
    pub channels: u16,
    /// Voices that can sound at once; a new voice beyond this replaces the oldest.
    pub max_voices: usize,
    /// Voices of the same sample (the same `Arc`) that can sound at once; a new one
    /// beyond this replaces the oldest of them.
    pub max_per_sample: usize,
    /// Commands that can wait for the next audio buffer.
    pub queue_capacity: usize,
    /// Voice-start reports that can wait for [`MixerHandle::drain_started`]; 0 turns
    /// reports off (ADR-0025).
    pub start_reports: usize,
}

impl Default for MixerConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            max_voices: 64,
            max_per_sample: SAMPLE_CONCURRENCY,
            queue_capacity: 256,
            start_reports: 0,
        }
    }
}

/// Why a command was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlayError {
    /// The audio thread has not taken the previous commands yet (stalled or closed
    /// stream). The command is dropped.
    #[error("mixer command queue is full")]
    QueueFull,
    /// The sample was not converted to the mixer's rate and channels.
    #[error("sample format does not match the mixer")]
    FormatMismatch,
    /// The rate is not finite or out of range: the track's
    /// [`MIN_MUSIC_RATE`](crate::MIN_MUSIC_RATE)..=[`MAX_MUSIC_RATE`](crate::MAX_MUSIC_RATE),
    /// a voice's 0..=[`MAX_VOICE_RATE`].
    #[error("rate out of range")]
    InvalidRate,
}

/// Mixer counters, written by the audio thread (relaxed), read by anyone.
#[derive(Debug, Default)]
pub struct MixerStats {
    /// Voices sounding after the last buffer.
    pub active_voices: AtomicU32,
    /// Voices started since creation.
    pub started_voices: AtomicU64,
    /// Voices cut off to make room for a new one.
    pub stolen_voices: AtomicU64,
    /// Buffers left silent because the stream format differs from the mixer's. Voices
    /// pause meanwhile.
    pub format_mismatches: AtomicU64,
    /// Sample references leaked because the release ring was full. Must stay 0: the
    /// ring is sized so that it cannot fill.
    pub release_overflows: AtomicU64,
    /// Voice-start reports dropped because nobody drained them in time.
    pub start_report_overflows: AtomicU64,
    /// Commands for a voice that had already ended or been stolen; ignored (ADR-0039).
    pub stale_voice_commands: AtomicU64,
}

/// A voice started with [`MixerHandle::play_voice`]: the number of its `play` command,
/// the same as [`VoiceStart::play`]. Commands for a voice that has ended or been
/// stolen do nothing (ADR-0039).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId(u64);

impl VoiceId {
    /// A voice that never sounds (an empty sample); commands for it are not queued.
    pub const NONE: Self = Self(u64::MAX);

    /// The voice of `play` command number `play`, for code that stands in for the mixer
    /// (recorders in tests). `u64::MAX` is [`VoiceId::NONE`].
    #[doc(hidden)]
    pub fn from_play(play: u64) -> Self {
        Self(play)
    }

    /// Number of the `play` command, as in [`VoiceStart::play`]; `None` for
    /// [`VoiceId::NONE`].
    pub fn play(self) -> Option<u64> {
        (self != Self::NONE).then_some(self.0)
    }
}

/// How a voice starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceParams {
    /// Linear gain, clamped to 0..=1 (non-finite counts as 0).
    pub gain: f32,
    /// Stereo balance, −1 (left) ..= 1 (right), clamped (non-finite counts as 0). The
    /// other side is attenuated as in BASS: left × `min(1, 1 − b)`, right ×
    /// `min(1, 1 + b)`. Channels after the second and mono streams are unaffected.
    pub balance: f32,
    /// Playback rate relative to the sample's own: 0 (paused) ..= [`MAX_VOICE_RATE`].
    /// Pitch changes with it. Rates below 2⁻³³ round to 0. Lazer's 100 Hz floor is the
    /// caller's to apply.
    pub rate: f64,
    /// Restarts from the first frame after the last until stopped.
    pub looping: bool,
}

impl Default for VoiceParams {
    fn default() -> Self {
        Self {
            gain: 1.0,
            balance: 0.0,
            rate: 1.0,
            looping: false,
        }
    }
}

/// When a queued `play` started to sound (ADR-0025).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceStart {
    /// Number of the `play` command, counting from 0 every command the handle queued;
    /// see [`MixerHandle::queued_plays`].
    pub play: u64,
    /// Stream frame the voice started at: the first frame of its buffer.
    pub frame: u64,
    /// [`CallbackInfo::timestamp_ns`] of that buffer; 0 when the backend had no timing.
    pub timestamp_ns: u64,
    /// [`CallbackInfo::latency_ns`] of that buffer.
    pub latency_ns: u64,
}

impl VoiceStart {
    /// Estimated time the first frame of the voice leaves the device, engine clock;
    /// `None` without backend timing.
    pub fn output_ns(&self) -> Option<u64> {
        (self.timestamp_ns != 0).then(|| self.timestamp_ns.saturating_add(self.latency_ns))
    }
}

enum Command {
    Play {
        sample: Arc<Sample>,
        gain: f32,
        balance: f32,
        step: u64,
        looping: bool,
    },
    Stop(u64),
    Gain {
        voice: u64,
        target: f32,
        frames: u32,
    },
    Balance {
        voice: u64,
        balance: f32,
    },
    Rate {
        voice: u64,
        step: u64,
    },
    StopAll,
    MasterGain(f32),
}

struct Voice {
    sample: Arc<Sample>,
    /// The `play` command that started it.
    id: u64,
    /// Next frame of the sample, 32.32 fixed point.
    pos: u64,
    /// Frames advanced per output frame, 32.32 fixed point.
    step: u64,
    looping: bool,
    gain: f32,
    /// Gain the ramp ends at.
    target: f32,
    /// Gain change per frame while `ramp_left > 0`.
    gain_step: f32,
    /// Frames left in the gain ramp; after them the gain is `target` exactly. The
    /// gain is `target - gain_step * ramp_left`, so rounding does not accumulate.
    ramp_left: u32,
    /// Gains of the first two channels from the balance.
    left: f32,
    right: f32,
}

impl Voice {
    fn set_balance(&mut self, balance: f32) {
        self.left = (1.0 - balance).min(1.0);
        self.right = (1.0 + balance).min(1.0);
    }

    /// Moves the gain ramp `frames` frames on.
    fn advance_ramp(&mut self, frames: u32) {
        if self.ramp_left > 0 {
            self.ramp_left = self.ramp_left.saturating_sub(frames);
            // Clamped: the product can overshoot the start by an ulp.
            self.gain = (self.target - self.gain_step * self.ramp_left as f32).clamp(0.0, 1.0);
        }
    }

    /// Adds this voice to `out` (whole frames of `channels`); returns whether it still
    /// sounds afterwards.
    fn mix(&mut self, out: &mut [f32], channels: usize, master_gain: f32) -> bool {
        if self.step == 0 {
            // Paused, as BASS pauses a channel at zero frequency. The gain ramp runs on
            // in real time, like lazer's `VolumeTo` transform on the update thread.
            self.advance_ramp((out.len() / channels).try_into().unwrap_or(u32::MAX));
            return true;
        }
        let simple = self.step == FRAME
            && self.pos & FRACTION == 0
            && self.ramp_left == 0
            && (channels < 2 || (self.left == 1.0 && self.right == 1.0));
        if simple {
            self.mix_simple(out, channels, master_gain)
        } else {
            // Constant channel counts let the frame loop unroll.
            match channels {
                1 => self.mix_general(out, 1, master_gain),
                2 => self.mix_general(out, 2, master_gain),
                n => self.mix_general(out, n, master_gain),
            }
        }
    }

    /// Rate 1, whole-frame cursor, constant gain, no balance: one multiply-add per
    /// value.
    fn mix_simple(&mut self, out: &mut [f32], channels: usize, master_gain: f32) -> bool {
        let data = self.sample.data();
        let gain = self.gain * master_gain;
        let mut cursor = (self.pos >> 32) as usize * channels;
        let mut written = 0;
        while written < out.len() {
            let n = (data.len() - cursor).min(out.len() - written);
            for (o, s) in out[written..written + n]
                .iter_mut()
                .zip(&data[cursor..cursor + n])
            {
                *o += s * gain;
            }
            written += n;
            cursor += n;
            if cursor == data.len() {
                if !self.looping {
                    return false;
                }
                cursor = 0;
            }
        }
        self.pos = ((cursor / channels) as u64) << 32;
        true
    }

    /// Any rate (linear interpolation between neighbouring frames), gain ramp and
    /// balance.
    #[inline(always)]
    fn mix_general(&mut self, out: &mut [f32], channels: usize, master_gain: f32) -> bool {
        let data = self.sample.data();
        let frames = self.sample.frames();
        let end = (frames as u64) << 32;
        // Balance touches the first two channels of a multichannel stream.
        let sides = if channels == 1 {
            [1.0, 1.0]
        } else {
            [self.left, self.right]
        };
        // Voices have the channels of their sample, which has at most `MAX_CHANNELS`.
        let silence = [0.0f32; MAX_CHANNELS as usize];
        for frame in out.chunks_exact_mut(channels) {
            if self.pos >= end {
                if !self.looping {
                    return false;
                }
                self.pos %= end;
            }
            let i = (self.pos >> 32) as usize;
            let t = (self.pos & FRACTION) as f32 * (1.0 / FRAME as f32);
            let a = &data[i * channels..(i + 1) * channels];
            let b = if i + 1 < frames {
                &data[(i + 1) * channels..(i + 2) * channels]
            } else if self.looping {
                &data[..channels]
            } else {
                // Past the end of a one-shot the next frame is silence.
                &silence[..channels]
            };
            let gain = self.gain * master_gain;
            for (c, ((o, &a), &b)) in frame.iter_mut().zip(a).zip(b).enumerate() {
                let side = if c < 2 { sides[c] } else { 1.0 };
                *o += (a + (b - a) * t) * (gain * side);
            }
            self.pos += self.step;
            // `advance_ramp(1)`, inline: `data` borrows the sample.
            if self.ramp_left > 0 {
                self.ramp_left -= 1;
                // Clamped: the product can overshoot the start by an ulp.
                self.gain = (self.target - self.gain_step * self.ramp_left as f32).clamp(0.0, 1.0);
            }
        }
        if self.looping {
            // The simple path expects a cursor inside the sample.
            self.pos %= end;
            true
        } else {
            self.pos < end
        }
    }
}

/// Creates a mixer for the audio stream and its handle for the update thread.
///
/// All memory is allocated here; neither side allocates afterwards.
pub fn mixer(config: &MixerConfig) -> Result<(Mixer, MixerHandle), AudioError> {
    if config.channels == 0 || config.sample_rate == 0 {
        return Err(AudioError::InvalidConfig("mixer needs a rate and channels"));
    }
    if config.max_voices == 0 || config.max_per_sample == 0 || config.queue_capacity == 0 {
        return Err(AudioError::InvalidConfig(
            "mixer needs at least one voice and one queued command",
        ));
    }
    let release_capacity = config
        .queue_capacity
        .checked_add(config.max_voices)
        .and_then(|n| n.checked_add(1))
        .ok_or(AudioError::InvalidConfig(
            "mixer queue and voices too large",
        ))?;
    let (command_tx, command_rx) = rtrb::RingBuffer::new(config.queue_capacity);
    // Every sample reference the mixer can hold is in the command ring, in a voice, or
    // in the release ring. The handle empties the release ring before each push, so at
    // most `queue_capacity + max_voices + 1` references are ever outstanding, and the
    // release ring never fills (see `MixerHandle::push`).
    let (release_tx, release_rx) = rtrb::RingBuffer::new(release_capacity);
    let (start_tx, start_rx) = if config.start_reports > 0 {
        let (tx, rx) = rtrb::RingBuffer::new(config.start_reports);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let stats = Arc::new(MixerStats::default());
    let mixer = Mixer {
        commands: command_rx,
        released: release_tx,
        starts: start_tx,
        plays: 0,
        voices: Vec::with_capacity(config.max_voices),
        max_voices: config.max_voices,
        max_per_sample: config.max_per_sample,
        master_gain: 1.0,
        sample_rate: config.sample_rate,
        channels: config.channels,
        music: None,
        stats: Arc::clone(&stats),
    };
    let handle = MixerHandle {
        commands: command_tx,
        released: release_rx,
        starts: start_rx,
        queued_plays: 0,
        sample_rate: config.sample_rate,
        channels: config.channels,
        stats,
    };
    Ok((mixer, handle))
}

/// Audio-thread side: an [`AudioCallback`] that mixes the active voices.
pub struct Mixer {
    commands: rtrb::Consumer<Command>,
    released: rtrb::Producer<Arc<Sample>>,
    starts: Option<rtrb::Producer<VoiceStart>>,
    /// `play` commands taken from the queue; numbers them in the order the handle
    /// counted them.
    plays: u64,
    /// Oldest first; never longer than `max_voices`, which its capacity covers.
    voices: Vec<Voice>,
    max_voices: usize,
    max_per_sample: usize,
    master_gain: f32,
    sample_rate: u32,
    channels: u16,
    /// The track, mixed under the voices (ADR-0033).
    music: Option<MusicSource>,
    stats: Arc<MixerStats>,
}

impl Mixer {
    /// Mixes `music` with the voices from now on. Call before handing the mixer to the
    /// stream; a source attached before replaces it (dropped here, not on the audio
    /// thread). The music must have the mixer's rate and channels.
    pub fn set_music(&mut self, music: MusicSource) -> Result<(), AudioError> {
        if music.format() != (self.sample_rate, self.channels) {
            return Err(AudioError::InvalidConfig(
                "music format differs from the mixer",
            ));
        }
        self.music = Some(music);
        Ok(())
    }

    /// The sounding voice started by `play` command `id`; counts a stale command when
    /// there is none.
    fn voice(&mut self, id: u64) -> Option<&mut Voice> {
        let found = self.voices.iter_mut().find(|v| v.id == id);
        if found.is_none() {
            self.stats
                .stale_voice_commands
                .fetch_add(1, Ordering::Relaxed);
        }
        found
    }

    fn stale(&self) {
        self.stats
            .stale_voice_commands
            .fetch_add(1, Ordering::Relaxed);
    }

    fn apply_commands(&mut self, info: &CallbackInfo) {
        // Only the commands present now: a producer pushing during the callback cannot
        // stretch it.
        for _ in 0..self.commands.slots() {
            let Ok(command) = self.commands.pop() else {
                break;
            };
            match command {
                Command::Play {
                    sample,
                    gain,
                    balance,
                    step,
                    looping,
                } => {
                    let mut same = 0;
                    let mut oldest_same = None;
                    for (i, voice) in self.voices.iter().enumerate() {
                        if Arc::ptr_eq(&voice.sample, &sample) {
                            same += 1;
                            oldest_same.get_or_insert(i);
                        }
                    }
                    let steal = match oldest_same {
                        Some(i) if same >= self.max_per_sample => Some(i),
                        _ if self.voices.len() >= self.max_voices => Some(0),
                        _ => None,
                    };
                    if let Some(i) = steal {
                        let stolen = self.voices.remove(i);
                        release(&mut self.released, &self.stats, stolen.sample);
                        self.stats.stolen_voices.fetch_add(1, Ordering::Relaxed);
                    }
                    let mut voice = Voice {
                        sample,
                        id: self.plays,
                        pos: 0,
                        step,
                        looping,
                        gain,
                        target: gain,
                        gain_step: 0.0,
                        ramp_left: 0,
                        left: 1.0,
                        right: 1.0,
                    };
                    voice.set_balance(balance);
                    self.voices.push(voice);
                    self.stats.started_voices.fetch_add(1, Ordering::Relaxed);
                    if let Some(starts) = &mut self.starts {
                        let report = VoiceStart {
                            play: self.plays,
                            frame: info.frame,
                            timestamp_ns: info.timestamp_ns,
                            latency_ns: info.latency_ns,
                        };
                        if starts.push(report).is_err() {
                            self.stats
                                .start_report_overflows
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    self.plays += 1;
                }
                Command::Stop(id) => {
                    if let Some(i) = self.voices.iter().position(|v| v.id == id) {
                        // `remove` keeps the oldest-first order.
                        let voice = self.voices.remove(i);
                        release(&mut self.released, &self.stats, voice.sample);
                    } else {
                        self.stale();
                    }
                }
                Command::Gain {
                    voice,
                    target,
                    frames,
                } => {
                    if let Some(voice) = self.voice(voice) {
                        // From the current gain, also mid-ramp.
                        voice.target = target;
                        voice.ramp_left = frames;
                        if frames == 0 {
                            voice.gain = target;
                        } else {
                            voice.gain_step = (target - voice.gain) / frames as f32;
                        }
                    }
                }
                Command::Balance { voice, balance } => {
                    if let Some(voice) = self.voice(voice) {
                        voice.set_balance(balance);
                    }
                }
                Command::Rate { voice, step } => {
                    if let Some(voice) = self.voice(voice) {
                        voice.step = step;
                    }
                }
                Command::StopAll => {
                    for voice in self.voices.drain(..) {
                        release(&mut self.released, &self.stats, voice.sample);
                    }
                }
                Command::MasterGain(gain) => self.master_gain = gain,
            }
        }
    }
}

impl AudioCallback for Mixer {
    fn process(&mut self, out: &mut [f32], info: &CallbackInfo) {
        // Commands queued before this buffer start at its first frame.
        self.apply_commands(info);
        out.fill(0.0);
        // Before the format check: the music applies its commands and counts its own
        // mismatches.
        if let Some(music) = &mut self.music {
            music.mix_into(out, info);
        }
        if info.channels != self.channels || info.sample_rate != self.sample_rate {
            self.stats.format_mismatches.fetch_add(1, Ordering::Relaxed);
            return;
        }

        let channels = usize::from(self.channels);
        let mut kept = 0;
        for i in 0..self.voices.len() {
            if self.voices[i].mix(out, channels, self.master_gain) {
                self.voices.swap(kept, i);
                kept += 1;
            }
        }
        // `drain` keeps the order of the voices before `kept`: oldest stays first.
        for voice in self.voices.drain(kept..) {
            release(&mut self.released, &self.stats, voice.sample);
        }
        for o in out.iter_mut() {
            *o = o.clamp(-1.0, 1.0);
        }
        self.stats
            .active_voices
            .store(self.voices.len() as u32, Ordering::Relaxed);
    }
}

/// Hands a sample reference back to the handle, which drops it off the audio thread.
fn release(released: &mut rtrb::Producer<Arc<Sample>>, stats: &MixerStats, sample: Arc<Sample>) {
    if let Err(rtrb::PushError::Full(sample)) = released.push(sample) {
        // Unreachable by the capacity bound in `mixer`. Leaking beats freeing memory
        // on the audio thread.
        stats.release_overflows.fetch_add(1, Ordering::Relaxed);
        std::mem::forget(sample);
    }
}

/// Update-thread side: queues commands for the mixer. Never blocks or allocates.
///
/// Keep the samples alive elsewhere (a sample bank) so that the references this handle
/// drops are plain counter decrements, not frees.
pub struct MixerHandle {
    commands: rtrb::Producer<Command>,
    released: rtrb::Consumer<Arc<Sample>>,
    starts: Option<rtrb::Consumer<VoiceStart>>,
    queued_plays: u64,
    sample_rate: u32,
    channels: u16,
    stats: Arc<MixerStats>,
}

impl MixerHandle {
    /// Starts `sample` at the first frame of the next audio buffer. `gain` is linear,
    /// clamped to 0..=1 (non-finite counts as 0). Copies of the same `Arc` beyond
    /// [`MixerConfig::max_per_sample`] replace its oldest copy, as in lazer. An empty
    /// sample is accepted and ignored.
    pub fn play(&mut self, sample: &Arc<Sample>, gain: f32) -> Result<(), PlayError> {
        self.play_voice(
            sample,
            VoiceParams {
                gain,
                ..VoiceParams::default()
            },
        )
        .map(|_| ())
    }

    /// Like [`play`](Self::play), with balance, rate and looping, and returns the
    /// voice to control it with. An empty sample gives [`VoiceId::NONE`].
    pub fn play_voice(
        &mut self,
        sample: &Arc<Sample>,
        params: VoiceParams,
    ) -> Result<VoiceId, PlayError> {
        if sample.sample_rate() != self.sample_rate || sample.channels() != self.channels {
            return Err(PlayError::FormatMismatch);
        }
        let step = rate_step(params.rate)?;
        if sample.data().is_empty() {
            // Nothing to hear; queuing it could only steal a sounding voice.
            return Ok(VoiceId::NONE);
        }
        self.push(Command::Play {
            sample: Arc::clone(sample),
            gain: clamp_gain(params.gain),
            balance: clamp_balance(params.balance),
            step,
            looping: params.looping,
        })?;
        let id = VoiceId(self.queued_plays);
        self.queued_plays += 1;
        Ok(id)
    }

    /// Stops `voice` at the start of the next audio buffer.
    pub fn stop_voice(&mut self, voice: VoiceId) -> Result<(), PlayError> {
        self.push_for(voice, Command::Stop)
    }

    /// Moves the gain of `voice` linearly to `gain` (clamped like
    /// [`VoiceParams::gain`]) over `ramp_ms` milliseconds of output from the next
    /// buffer, starting at its current gain; 0 (or not finite) sets it at once.
    pub fn set_voice_gain(
        &mut self,
        voice: VoiceId,
        gain: f32,
        ramp_ms: f64,
    ) -> Result<(), PlayError> {
        let frames = if ramp_ms.is_finite() && ramp_ms > 0.0 {
            // Saturating cast: absurd ramps just never finish.
            (ramp_ms * f64::from(self.sample_rate) / 1000.0).round() as u32
        } else {
            0
        };
        let target = clamp_gain(gain);
        self.push_for(voice, |voice| Command::Gain {
            voice,
            target,
            frames,
        })
    }

    /// Sets the balance of `voice` (see [`VoiceParams::balance`]) from the next buffer.
    pub fn set_voice_balance(&mut self, voice: VoiceId, balance: f32) -> Result<(), PlayError> {
        let balance = clamp_balance(balance);
        self.push_for(voice, |voice| Command::Balance { voice, balance })
    }

    /// Sets the rate of `voice` (see [`VoiceParams::rate`]) from the next buffer.
    pub fn set_voice_rate(&mut self, voice: VoiceId, rate: f64) -> Result<(), PlayError> {
        let step = rate_step(rate)?;
        self.push_for(voice, |voice| Command::Rate { voice, step })
    }

    /// `play` commands queued so far (empty samples and rejected commands do not
    /// count). The last queued one has number `queued_plays() - 1` in [`VoiceStart`].
    pub fn queued_plays(&self) -> u64 {
        self.queued_plays
    }

    /// Calls `f` on every voice-start report the mixer has made, oldest first. Does
    /// nothing when [`MixerConfig::start_reports`] is 0.
    pub fn drain_started(&mut self, mut f: impl FnMut(VoiceStart)) {
        if let Some(starts) = &mut self.starts {
            while let Ok(report) = starts.pop() {
                f(report);
            }
        }
    }

    /// Silences every voice at the start of the next audio buffer.
    pub fn stop_all(&mut self) -> Result<(), PlayError> {
        self.push(Command::StopAll)
    }

    /// Sets the gain applied to all voices from the next audio buffer; linear, clamped
    /// to 0..=1.
    pub fn set_master_gain(&mut self, gain: f32) -> Result<(), PlayError> {
        self.push(Command::MasterGain(clamp_gain(gain)))
    }

    /// Drops the sample references the mixer has finished with. Every command does
    /// this first; call it when idle to release samples sooner.
    pub fn collect(&mut self) {
        while let Ok(sample) = self.released.pop() {
            drop(sample);
        }
    }

    /// Mixer counters.
    pub fn stats(&self) -> &Arc<MixerStats> {
        &self.stats
    }

    fn push_for(
        &mut self,
        voice: VoiceId,
        command: impl FnOnce(u64) -> Command,
    ) -> Result<(), PlayError> {
        match voice.play() {
            Some(id) => self.push(command(id)),
            None => Ok(()),
        }
    }

    fn push(&mut self, command: Command) -> Result<(), PlayError> {
        // Emptying the release ring before every push bounds the references the mixer
        // can hold; the ring capacity relies on it.
        self.collect();
        self.commands
            .push(command)
            .map_err(|rtrb::PushError::Full(_)| PlayError::QueueFull)
    }
}

fn clamp_balance(balance: f32) -> f32 {
    if balance.is_finite() {
        balance.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// The 32.32 cursor step for `rate`.
fn rate_step(rate: f64) -> Result<u64, PlayError> {
    if !(0.0..=MAX_VOICE_RATE).contains(&rate) {
        return Err(PlayError::InvalidRate);
    }
    Ok((rate * FRAME as f64).round() as u64)
}

fn clamp_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

impl std::fmt::Debug for Mixer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mixer")
            .field("voices", &self.voices.len())
            .field("master_gain", &self.master_gain)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for MixerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MixerHandle")
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn info(channels: u16) -> CallbackInfo {
        CallbackInfo {
            frame: 0,
            timestamp_ns: 0,
            latency_ns: 0,
            sample_rate: RATE,
            channels,
        }
    }

    fn sample(data: &[f32], channels: u16) -> Arc<Sample> {
        Arc::new(Sample::from_interleaved(data.to_vec(), RATE, channels).unwrap())
    }

    fn mono_mixer(max_voices: usize, queue_capacity: usize) -> (Mixer, MixerHandle) {
        mixer(&MixerConfig {
            sample_rate: RATE,
            channels: 1,
            max_voices,
            queue_capacity,
            ..Default::default()
        })
        .unwrap()
    }

    fn run(mixer: &mut Mixer, frames: usize) -> Vec<f32> {
        let mut out = vec![9.0; frames];
        mixer.process(&mut out, &info(1));
        out
    }

    #[test]
    fn silent_without_voices() {
        let (mut m, _h) = mono_mixer(4, 4);
        assert_eq!(run(&mut m, 4), [0.0; 4]);
    }

    #[test]
    fn plays_sample_from_next_buffer_with_gain() {
        let (mut m, mut h) = mono_mixer(4, 4);
        h.play(&sample(&[0.5, -0.5, 0.25], 1), 0.5).unwrap();
        assert_eq!(run(&mut m, 5), [0.25, -0.25, 0.125, 0.0, 0.0]);
        assert_eq!(m.stats.active_voices.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn voice_continues_across_buffers() {
        let (mut m, mut h) = mono_mixer(4, 4);
        h.play(&sample(&[0.1, 0.2, 0.3, 0.4, 0.5], 1), 1.0).unwrap();
        assert_eq!(run(&mut m, 2), [0.1, 0.2]);
        assert_eq!(m.stats.active_voices.load(Ordering::Relaxed), 1);
        assert_eq!(run(&mut m, 2), [0.3, 0.4]);
        assert_eq!(run(&mut m, 2), [0.5, 0.0]);
        assert_eq!(m.stats.active_voices.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn voices_sum_and_clip() {
        let (mut m, mut h) = mono_mixer(4, 4);
        let s = sample(&[0.25, 0.75, -0.75], 1);
        h.play(&s, 1.0).unwrap();
        h.play(&s, 1.0).unwrap();
        assert_eq!(run(&mut m, 3), [0.5, 1.0, -1.0]);
    }

    #[test]
    fn stereo_frames_stay_interleaved() {
        let (mut m, mut h) = mixer(&MixerConfig {
            sample_rate: RATE,
            channels: 2,
            max_voices: 2,
            queue_capacity: 2,
            ..Default::default()
        })
        .unwrap();
        h.play(&sample(&[0.1, -0.1, 0.2, -0.2], 2), 1.0).unwrap();
        let mut out = [9.0; 6];
        m.process(&mut out, &info(2));
        assert_eq!(out, [0.1, -0.1, 0.2, -0.2, 0.0, 0.0]);
    }

    #[test]
    fn full_pool_steals_oldest_voice() {
        let (mut m, mut h) = mono_mixer(2, 4);
        h.play(&sample(&[0.1; 4], 1), 1.0).unwrap();
        h.play(&sample(&[0.2; 4], 1), 1.0).unwrap();
        run(&mut m, 1);
        h.play(&sample(&[0.4; 4], 1), 1.0).unwrap();
        let out = run(&mut m, 1);
        assert!((out[0] - 0.6).abs() < 1e-6, "{out:?}");
        assert_eq!(m.stats.stolen_voices.load(Ordering::Relaxed), 1);
        assert_eq!(m.stats.started_voices.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn stop_all_and_master_gain() {
        let (mut m, mut h) = mono_mixer(4, 4);
        let s = sample(&[0.5; 8], 1);
        h.play(&s, 1.0).unwrap();
        h.set_master_gain(0.5).unwrap();
        assert_eq!(run(&mut m, 1), [0.25]);
        h.stop_all().unwrap();
        assert_eq!(run(&mut m, 2), [0.0, 0.0]);
        h.play(&s, 1.0).unwrap();
        assert_eq!(run(&mut m, 1), [0.25]);
    }

    #[test]
    fn rejects_mismatched_sample_and_full_queue() {
        let (_m, mut h) = mono_mixer(4, 1);
        let wrong_channels = sample(&[0.0, 0.0], 2);
        assert_eq!(h.play(&wrong_channels, 1.0), Err(PlayError::FormatMismatch));
        let wrong_rate = Arc::new(Sample::from_interleaved(vec![0.0], 44_100, 1).unwrap());
        assert_eq!(h.play(&wrong_rate, 1.0), Err(PlayError::FormatMismatch));
        let s = sample(&[0.0], 1);
        h.play(&s, 1.0).unwrap();
        assert_eq!(h.play(&s, 1.0), Err(PlayError::QueueFull));
        // The rejected command released its reference at once.
        assert_eq!(Arc::strong_count(&s), 2);
    }

    #[test]
    fn gain_is_clamped() {
        let (mut m, mut h) = mono_mixer(4, 4);
        let s = sample(&[0.5], 1);
        h.play(&s, f32::NAN).unwrap();
        h.play(&s, -1.0).unwrap();
        h.play(&s, 3.0).unwrap();
        assert_eq!(run(&mut m, 1), [0.5]);
    }

    #[test]
    fn stream_format_mismatch_is_silent() {
        let (mut m, mut h) = mono_mixer(4, 4);
        h.play(&sample(&[0.5; 4], 1), 1.0).unwrap();
        let mut out = [9.0; 4];
        m.process(&mut out, &info(2));
        assert_eq!(out, [0.0; 4]);
        assert_eq!(m.stats.format_mismatches.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn finished_samples_are_dropped_by_the_handle() {
        let (mut m, mut h) = mono_mixer(2, 2);
        let s = sample(&[0.5], 1);
        h.play(&s, 1.0).unwrap();
        run(&mut m, 4);
        // The mixer gave its reference back but did not drop it.
        assert_eq!(Arc::strong_count(&s), 2);
        h.collect();
        assert_eq!(Arc::strong_count(&s), 1);
    }

    #[test]
    fn release_ring_never_overflows() {
        // Worst case for the bound: a full pool and a full queue of stealing plays,
        // with the handle draining only through its pushes.
        let (mut m, mut h) = mono_mixer(3, 2);
        let s = sample(&[0.0; 64], 1);
        for _ in 0..1_000 {
            while h.play(&s, 1.0).is_ok() {}
            run(&mut m, 1);
            h.stop_all().ok();
            while h.play(&s, 1.0).is_ok() {}
            run(&mut m, 64);
        }
        assert_eq!(m.stats.release_overflows.load(Ordering::Relaxed), 0);
        drop(m);
        h.collect();
        assert_eq!(Arc::strong_count(&s), 1);
    }

    #[test]
    fn copies_of_one_sample_are_limited_like_lazer() {
        let (mut m, mut h) = mono_mixer(16, 16);
        assert_eq!(MixerConfig::default().max_per_sample, SAMPLE_CONCURRENCY);
        let other = sample(&[0.01; 8], 1);
        h.play(&other, 1.0).unwrap();
        // Each copy is louder than the last, to tell which ones still sound.
        let s = sample(&[0.1; 8], 1);
        for i in 0..SAMPLE_CONCURRENCY + 1 {
            h.play(&s, 0.1 * (i + 1) as f32).unwrap();
        }
        let out = run(&mut m, 1);
        // The first copy (gain 0.1) was replaced; `other` was not touched.
        let gains: f32 = (2..=SAMPLE_CONCURRENCY + 1).map(|g| 0.1 * g as f32).sum();
        assert!((out[0] - (0.01 + 0.1 * gains)).abs() < 1e-5, "{out:?}");
        assert_eq!(m.stats.stolen_voices.load(Ordering::Relaxed), 1);
        assert_eq!(
            m.stats.active_voices.load(Ordering::Relaxed),
            SAMPLE_CONCURRENCY as u32 + 1
        );
    }

    #[test]
    fn burst_beyond_the_pool_keeps_the_newest() {
        let (mut m, mut h) = mono_mixer(2, 8);
        for level in [0.1, 0.2, 0.3, 0.4] {
            h.play(&sample(&[level; 4], 1), 1.0).unwrap();
        }
        let out = run(&mut m, 1);
        assert!((out[0] - 0.7).abs() < 1e-6, "{out:?}");
        assert_eq!(m.stats.stolen_voices.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn finished_voice_in_the_middle_keeps_age_order() {
        let (mut m, mut h) = mono_mixer(3, 8);
        h.play(&sample(&[0.1; 4], 1), 1.0).unwrap();
        h.play(&sample(&[0.2; 1], 1), 1.0).unwrap();
        h.play(&sample(&[0.4; 4], 1), 1.0).unwrap();
        run(&mut m, 1);
        // The middle voice ended; two new voices fill the pool and then steal the
        // oldest (0.1), not the newer 0.4.
        h.play(&sample(&[0.8; 4], 1), 0.1).unwrap();
        h.play(&sample(&[0.8; 4], 1), 0.1).unwrap();
        let out = run(&mut m, 1);
        assert!((out[0] - (0.4 + 0.16)).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn empty_sample_and_empty_buffer() {
        let (mut m, mut h) = mono_mixer(1, 2);
        let long = sample(&[0.5; 4], 1);
        h.play(&long, 1.0).unwrap();
        // An empty sample neither queues nor steals the only voice.
        h.play(&sample(&[], 1), 1.0).unwrap();
        run(&mut m, 0);
        assert_eq!(m.stats.started_voices.load(Ordering::Relaxed), 1);
        assert_eq!(run(&mut m, 2), [0.5, 0.5]);
    }

    fn reporting_mixer(queue_capacity: usize, start_reports: usize) -> (Mixer, MixerHandle) {
        mixer(&MixerConfig {
            sample_rate: RATE,
            channels: 1,
            max_voices: 2,
            queue_capacity,
            start_reports,
            ..Default::default()
        })
        .unwrap()
    }

    fn started(h: &mut MixerHandle) -> Vec<VoiceStart> {
        let mut reports = Vec::new();
        h.drain_started(|r| reports.push(r));
        reports
    }

    #[test]
    fn reports_voice_starts_with_buffer_timing() {
        let (mut m, mut h) = reporting_mixer(8, 8);
        let s = sample(&[0.5; 4], 1);
        h.play(&s, 1.0).unwrap();
        assert_eq!(h.queued_plays(), 1);
        let timing = CallbackInfo {
            frame: 480,
            timestamp_ns: 1_000_000,
            latency_ns: 250_000,
            ..info(1)
        };
        m.process(&mut [0.0; 2], &timing);
        // Two more in one buffer, one of them stealing: each still gets a report.
        h.play(&s, 1.0).unwrap();
        h.play(&s, 1.0).unwrap();
        m.process(
            &mut [0.0; 2],
            &CallbackInfo {
                frame: 482,
                ..timing
            },
        );
        let reports = started(&mut h);
        assert_eq!(
            reports
                .iter()
                .map(|r| (r.play, r.frame))
                .collect::<Vec<_>>(),
            [(0, 480), (1, 482), (2, 482)]
        );
        assert_eq!(reports[0].output_ns(), Some(1_250_000));
        assert_eq!(h.queued_plays(), 3);
        assert!(started(&mut h).is_empty());
    }

    #[test]
    fn play_numbers_skip_rejected_and_empty_plays() {
        let (mut m, mut h) = reporting_mixer(1, 8);
        let s = sample(&[0.5], 1);
        h.play(&sample(&[], 1), 1.0).unwrap();
        h.play(&s, 1.0).unwrap();
        assert_eq!(h.play(&s, 1.0), Err(PlayError::QueueFull));
        assert_eq!(
            h.play(&sample(&[0.0; 2], 2), 1.0),
            Err(PlayError::FormatMismatch)
        );
        assert_eq!(h.queued_plays(), 1);
        run(&mut m, 1);
        h.play(&s, 1.0).unwrap();
        run(&mut m, 1);
        let plays: Vec<u64> = started(&mut h).iter().map(|r| r.play).collect();
        assert_eq!(plays, [0, 1]);
        // No backend timing yet: no output estimate.
        assert_eq!(
            VoiceStart {
                play: 0,
                frame: 0,
                timestamp_ns: 0,
                latency_ns: 5
            }
            .output_ns(),
            None
        );
    }

    #[test]
    fn full_report_ring_drops_reports_not_voices() {
        let (mut m, mut h) = reporting_mixer(8, 2);
        let s = sample(&[0.25; 4], 1);
        for _ in 0..3 {
            h.play(&sample(&[0.25; 4], 1), 1.0).unwrap();
        }
        h.play(&s, 1.0).unwrap();
        run(&mut m, 1);
        assert_eq!(m.stats.started_voices.load(Ordering::Relaxed), 4);
        assert_eq!(m.stats.start_report_overflows.load(Ordering::Relaxed), 2);
        let plays: Vec<u64> = started(&mut h).iter().map(|r| r.play).collect();
        assert_eq!(plays, [0, 1]);
        // Numbering stays in step after the drops.
        h.play(&s, 1.0).unwrap();
        run(&mut m, 1);
        assert_eq!(started(&mut h)[0].play, 4);
    }

    #[test]
    fn other_commands_do_not_take_play_numbers() {
        let (mut m, mut h) = reporting_mixer(8, 8);
        let s = sample(&[0.5; 4], 1);
        h.play(&s, 1.0).unwrap();
        h.stop_all().unwrap();
        h.play(&s, 1.0).unwrap();
        h.set_master_gain(0.5).unwrap();
        h.play(&s, 1.0).unwrap();
        run(&mut m, 1);
        let plays: Vec<u64> = started(&mut h).iter().map(|r| r.play).collect();
        assert_eq!(plays, [0, 1, 2]);
        assert_eq!(h.queued_plays(), 3);
    }

    #[test]
    fn play_in_a_mismatched_buffer_is_reported_there() {
        // ADR-0025: the report names the silent buffer; the voice sounds from the
        // next matching one.
        let (mut m, mut h) = reporting_mixer(8, 8);
        h.play(&sample(&[0.5; 4], 1), 1.0).unwrap();
        m.process(
            &mut [0.0; 4],
            &CallbackInfo {
                frame: 7,
                ..info(2)
            },
        );
        let reports = started(&mut h);
        assert_eq!(
            reports
                .iter()
                .map(|r| (r.play, r.frame))
                .collect::<Vec<_>>(),
            [(0, 7)]
        );
        assert_eq!(run(&mut m, 1), [0.5]);
    }

    #[test]
    fn reports_are_off_by_default() {
        let (mut m, mut h) = mono_mixer(2, 2);
        h.play(&sample(&[0.5], 1), 1.0).unwrap();
        run(&mut m, 1);
        assert!(started(&mut h).is_empty());
        assert_eq!(h.queued_plays(), 1);
        assert_eq!(m.stats.start_report_overflows.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn music_is_mixed_under_the_voices_and_clipped_with_them() {
        use crate::music::{MusicConfig, music};
        let (mut m, mut h) = mono_mixer(4, 4);
        let (source, mut player) = music(&MusicConfig {
            sample_rate: RATE,
            channels: 1,
            ..Default::default()
        })
        .unwrap();
        m.set_music(source).unwrap();
        // Without a track the music adds nothing but still publishes its position.
        h.play(&sample(&[0.5, 0.75], 1), 1.0).unwrap();
        assert_eq!(run(&mut m, 3), [0.5, 0.75, 0.0]);
        assert!(player.position().read().is_some());
        player.play().unwrap();
        // Stream format differs from both: silent, counted by both.
        let mut out = [9.0; 4];
        m.process(&mut out, &info(2));
        assert_eq!(out, [0.0; 4]);
        assert_eq!(m.stats.format_mismatches.load(Ordering::Relaxed), 1);
        assert_eq!(player.stats().format_mismatches.load(Ordering::Relaxed), 1);
    }

    fn played(h: &mut MixerHandle, data: &[f32], params: VoiceParams) -> VoiceId {
        h.play_voice(&sample(data, 1), params).unwrap()
    }

    fn assert_close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-6, "{actual:?} != {expected:?}");
        }
    }

    const LOOP: VoiceParams = VoiceParams {
        gain: 1.0,
        balance: 0.0,
        rate: 1.0,
        looping: true,
    };

    #[test]
    fn looping_voice_wraps_until_stopped() {
        let (mut m, mut h) = mono_mixer(4, 4);
        let s = sample(&[0.1, 0.2, 0.3], 1);
        let v = h.play_voice(&s, LOOP).unwrap();
        assert_eq!(run(&mut m, 7), [0.1, 0.2, 0.3, 0.1, 0.2, 0.3, 0.1]);
        assert_eq!(run(&mut m, 2), [0.2, 0.3]);
        assert_eq!(m.stats.active_voices.load(Ordering::Relaxed), 1);
        h.stop_voice(v).unwrap();
        assert_eq!(run(&mut m, 2), [0.0; 2]);
        assert_eq!(m.stats.active_voices.load(Ordering::Relaxed), 0);
        h.collect();
        assert_eq!(Arc::strong_count(&s), 1);
        assert_eq!(m.stats.stale_voice_commands.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn stopping_one_voice_keeps_the_others() {
        let (mut m, mut h) = mono_mixer(4, 4);
        let a = played(&mut h, &[0.5; 4], LOOP);
        played(&mut h, &[0.25; 4], LOOP);
        assert_eq!(run(&mut m, 1), [0.75]);
        h.stop_voice(a).unwrap();
        assert_eq!(run(&mut m, 1), [0.25]);
    }

    #[test]
    fn voice_ids_are_play_numbers() {
        let (_m, mut h) = mono_mixer(4, 4);
        h.play(&sample(&[0.1], 1), 1.0).unwrap();
        let v = played(&mut h, &[0.1], VoiceParams::default());
        assert_eq!(v.play(), Some(1));
        assert_eq!(h.queued_plays(), 2);
    }

    #[test]
    fn commands_for_stolen_or_finished_voices_do_nothing() {
        let (mut m, mut h) = mono_mixer(1, 8);
        let stolen = played(&mut h, &[0.5; 8], LOOP);
        let kept = played(&mut h, &[0.25; 8], LOOP);
        assert_eq!(run(&mut m, 1), [0.25]);
        h.stop_voice(stolen).unwrap();
        h.set_voice_gain(stolen, 0.0, 0.0).unwrap();
        h.set_voice_balance(stolen, 1.0).unwrap();
        h.set_voice_rate(stolen, 2.0).unwrap();
        assert_eq!(run(&mut m, 1), [0.25]);
        assert_eq!(m.stats.stale_voice_commands.load(Ordering::Relaxed), 4);

        h.stop_voice(kept).unwrap();
        let short = played(&mut h, &[0.5], VoiceParams::default());
        assert_eq!(run(&mut m, 2), [0.5, 0.0]);
        h.stop_voice(short).unwrap();
        run(&mut m, 1);
        assert_eq!(m.stats.stale_voice_commands.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn empty_sample_gives_a_voice_that_takes_no_commands() {
        let (mut m, mut h) = mono_mixer(4, 1);
        let v = h.play_voice(&sample(&[], 1), LOOP).unwrap();
        assert_eq!(v, VoiceId::NONE);
        assert_eq!(v.play(), None);
        // Not queued: the single slot stays free.
        h.stop_voice(v).unwrap();
        h.set_voice_gain(v, 0.5, 10.0).unwrap();
        h.play(&sample(&[0.5], 1), 1.0).unwrap();
        assert_eq!(run(&mut m, 1), [0.5]);
        assert_eq!(h.queued_plays(), 1);
        assert_eq!(m.stats.stale_voice_commands.load(Ordering::Relaxed), 0);
    }

    /// Milliseconds of `frames` at the test rate.
    fn ms(frames: u32) -> f64 {
        f64::from(frames) * 1000.0 / f64::from(RATE)
    }

    #[test]
    fn gain_ramps_linearly_per_frame() {
        let (mut m, mut h) = mono_mixer(4, 4);
        let v = played(&mut h, &[1.0; 64], VoiceParams { gain: 0.0, ..LOOP });
        h.set_voice_gain(v, 1.0, ms(4)).unwrap();
        assert_eq!(run(&mut m, 3), [0.0, 0.25, 0.5]);
        assert_eq!(run(&mut m, 3), [0.75, 1.0, 1.0]);
        // A new target mid-ramp starts from the current gain.
        h.set_voice_gain(v, 0.0, ms(4)).unwrap();
        assert_eq!(run(&mut m, 2), [1.0, 0.75]);
        h.set_voice_gain(v, 1.0, ms(2)).unwrap();
        assert_eq!(run(&mut m, 4), [0.5, 0.75, 1.0, 1.0]);
        h.set_voice_gain(v, 0.25, 0.0).unwrap();
        assert_eq!(run(&mut m, 2), [0.25, 0.25]);
        // Clamped like the start gain; non-finite ramps are instant.
        h.set_voice_gain(v, 2.0, f64::NAN).unwrap();
        assert_eq!(run(&mut m, 1), [1.0]);
    }

    #[test]
    fn gain_ramp_applies_master_gain() {
        let (mut m, mut h) = mono_mixer(4, 4);
        let v = played(&mut h, &[1.0; 64], LOOP);
        h.set_master_gain(0.5).unwrap();
        h.set_voice_gain(v, 0.0, ms(2)).unwrap();
        assert_eq!(run(&mut m, 3), [0.5, 0.25, 0.0]);
    }

    #[test]
    fn balance_attenuates_the_other_side() {
        let (mut m, mut h) = mixer(&MixerConfig {
            channels: 2,
            max_voices: 4,
            queue_capacity: 4,
            ..Default::default()
        })
        .unwrap();
        let s = sample(&[1.0; 2 * 64], 2);
        let v = h
            .play_voice(
                &s,
                VoiceParams {
                    balance: -0.5,
                    ..LOOP
                },
            )
            .unwrap();
        let mut out = [0.0; 4];
        m.process(&mut out, &info(2));
        assert_eq!(out, [1.0, 0.5, 1.0, 0.5]);
        h.set_voice_balance(v, 0.25).unwrap();
        m.process(&mut out, &info(2));
        assert_eq!(out, [0.75, 1.0, 0.75, 1.0]);
        h.set_voice_balance(v, 7.0).unwrap();
        m.process(&mut out, &info(2));
        assert_eq!(out, [0.0, 1.0, 0.0, 1.0]);
        h.set_voice_balance(v, f32::NAN).unwrap();
        m.process(&mut out, &info(2));
        assert_eq!(out, [1.0; 4]);
    }

    #[test]
    fn balance_leaves_mono_alone() {
        let (mut m, mut h) = mono_mixer(4, 4);
        played(
            &mut h,
            &[0.5; 4],
            VoiceParams {
                balance: 1.0,
                ..VoiceParams::default()
            },
        );
        assert_eq!(run(&mut m, 2), [0.5, 0.5]);
    }

    const RAMP: [f32; 8] = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7];

    #[test]
    fn faster_rate_skips_frames_and_ends_sooner() {
        let (mut m, mut h) = mono_mixer(4, 4);
        played(
            &mut h,
            &RAMP,
            VoiceParams {
                rate: 2.0,
                ..VoiceParams::default()
            },
        );
        assert_close(&run(&mut m, 5), &[0.0, 0.2, 0.4, 0.6, 0.0]);
        assert_eq!(m.stats.active_voices.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn slower_rate_interpolates_and_fades_past_the_end() {
        let (mut m, mut h) = mono_mixer(4, 4);
        played(
            &mut h,
            &[0.2, 0.4],
            VoiceParams {
                rate: 0.5,
                ..VoiceParams::default()
            },
        );
        assert_close(&run(&mut m, 5), &[0.2, 0.3, 0.4, 0.2, 0.0]);
        assert_eq!(m.stats.active_voices.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn looping_at_a_fractional_rate_interpolates_across_the_seam() {
        let (mut m, mut h) = mono_mixer(4, 4);
        played(&mut h, &RAMP[..4], VoiceParams { rate: 1.5, ..LOOP });
        // Cursor 0, 1.5, 3, 4.5 → 0.5, 2, 3.5 (between the last and the first frame).
        assert_close(&run(&mut m, 6), &[0.0, 0.15, 0.3, 0.05, 0.2, 0.15]);
        // The cursor wrapped to 5 % 4 = 1 at the end of the buffer.
        assert_close(&run(&mut m, 2), &[0.1, 0.25]);
    }

    #[test]
    fn rate_changes_apply_from_the_next_buffer() {
        let (mut m, mut h) = mono_mixer(4, 4);
        let v = played(&mut h, &RAMP, VoiceParams::default());
        assert_close(&run(&mut m, 2), &[0.0, 0.1]);
        h.set_voice_rate(v, 2.0).unwrap();
        assert_close(&run(&mut m, 2), &[0.2, 0.4]);
        h.set_voice_rate(v, 1.0).unwrap();
        assert_close(&run(&mut m, 2), &[0.6, 0.7]);
    }

    #[test]
    fn zero_rate_pauses_silently() {
        let (mut m, mut h) = mono_mixer(4, 4);
        let v = played(&mut h, &RAMP, VoiceParams::default());
        assert_close(&run(&mut m, 2), &[0.0, 0.1]);
        h.set_voice_rate(v, 0.0).unwrap();
        h.set_voice_gain(v, 0.5, ms(2)).unwrap();
        assert_eq!(run(&mut m, 3), [0.0; 3]);
        assert_eq!(m.stats.active_voices.load(Ordering::Relaxed), 1);
        // The ramp ran on in real time, like lazer's `VolumeTo`.
        h.set_voice_rate(v, 1.0).unwrap();
        assert_close(&run(&mut m, 3), &[0.1, 0.15, 0.2]);
    }

    #[test]
    fn rejects_rates_out_of_range() {
        let (_m, mut h) = mono_mixer(4, 4);
        let s = sample(&[0.1], 1);
        for rate in [-0.5, MAX_VOICE_RATE + 0.1, f64::NAN, f64::INFINITY] {
            let params = VoiceParams {
                rate,
                ..VoiceParams::default()
            };
            assert_eq!(h.play_voice(&s, params), Err(PlayError::InvalidRate));
        }
        let v = h.play_voice(&s, VoiceParams::default()).unwrap();
        assert_eq!(h.set_voice_rate(v, f64::NAN), Err(PlayError::InvalidRate));
        assert_eq!(h.queued_plays(), 1);
    }

    #[test]
    fn one_frame_loop_at_top_rate_wraps_many_times_per_frame() {
        let (mut m, mut h) = mono_mixer(4, 4);
        played(&mut h, &[0.5], VoiceParams { rate: 8.0, ..LOOP });
        assert_eq!(run(&mut m, 4), [0.5; 4]);
        assert_eq!(m.stats.active_voices.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn one_shot_ending_on_a_buffer_boundary_in_the_general_path() {
        let (mut m, mut h) = mono_mixer(4, 4);
        played(
            &mut h,
            &RAMP,
            VoiceParams {
                rate: 2.0,
                ..VoiceParams::default()
            },
        );
        assert_close(&run(&mut m, 4), &[0.0, 0.2, 0.4, 0.6]);
        assert_eq!(m.stats.active_voices.load(Ordering::Relaxed), 0);
        assert_eq!(run(&mut m, 2), [0.0; 2]);
    }

    #[test]
    fn looping_voice_returns_to_the_simple_path_at_rate_one() {
        let (mut m, mut h) = mono_mixer(4, 4);
        let v = played(&mut h, &RAMP[..4], VoiceParams { rate: 0.5, ..LOOP });
        assert_close(&run(&mut m, 2), &[0.0, 0.05]);
        h.set_voice_rate(v, 1.0).unwrap();
        // Cursor 1: whole again, so the simple path continues across the seam.
        assert_close(&run(&mut m, 5), &[0.1, 0.2, 0.3, 0.0, 0.1]);
        let voice = &m.voices[0];
        assert_eq!((voice.pos, voice.step), (2 * FRAME, FRAME));
    }

    #[test]
    fn stereo_interpolation_keeps_channels_apart() {
        let (mut m, mut h) = mixer(&MixerConfig {
            channels: 2,
            max_voices: 4,
            queue_capacity: 4,
            ..Default::default()
        })
        .unwrap();
        let s = sample(&[0.2, -0.2, 0.4, -0.4], 2);
        h.play_voice(
            &s,
            VoiceParams {
                rate: 0.5,
                balance: 0.5,
                ..VoiceParams::default()
            },
        )
        .unwrap();
        let mut out = [0.0; 8];
        m.process(&mut out, &info(2));
        assert_close(&out, &[0.1, -0.2, 0.15, -0.3, 0.2, -0.4, 0.1, -0.2]);
    }

    #[test]
    fn commands_after_stop_all_are_stale() {
        let (mut m, mut h) = mono_mixer(4, 4);
        let v = played(&mut h, &[0.5; 4], LOOP);
        h.stop_all().unwrap();
        h.set_voice_balance(v, 0.5).unwrap();
        assert_eq!(run(&mut m, 2), [0.0; 2]);
        assert_eq!(m.stats.stale_voice_commands.load(Ordering::Relaxed), 1);
    }

    /// Random voices, commands and buffer sizes: no panic, finite output, every
    /// sample reference back to the bank in the end.
    #[test]
    fn random_voice_commands_stay_in_bounds() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        const RATES: [f64; 9] = [0.0, 1e-9, 0.333, 0.5, 1.0, 1.5, 2.0, 7.999, 8.0];
        for channels in [1u16, 2, 3, MAX_CHANNELS] {
            let (mut m, mut h) = mixer(&MixerConfig {
                channels,
                max_voices: 6,
                max_per_sample: 3,
                queue_capacity: 64,
                ..Default::default()
            })
            .unwrap();
            let bank: Vec<_> = [1usize, 2, 3, 7, 64]
                .iter()
                .map(|&frames| {
                    let data: Vec<f32> = (0..frames * usize::from(channels))
                        .map(|i| (i % 5) as f32 * 0.1 - 0.2)
                        .collect();
                    sample(&data, channels)
                })
                .collect();
            let mut voices = Vec::new();
            let mut out = vec![0.0; 8 * usize::from(channels)];
            let info = info(channels);
            for _ in 0..20_000 {
                let r = next();
                let rate = RATES[(r >> 8) as usize % RATES.len()];
                let voice = voices
                    .get((r >> 16) as usize % voices.len().max(1))
                    .copied()
                    .unwrap_or(VoiceId::NONE);
                let _ = match r % 6 {
                    0 => {
                        let s = &bank[(r >> 24) as usize % bank.len()];
                        let params = VoiceParams {
                            gain: ((r >> 28) % 3) as f32 * 0.5,
                            balance: ((r >> 32) % 5) as f32 * 0.5 - 1.0,
                            rate,
                            looping: r & 0x40 != 0,
                        };
                        h.play_voice(s, params).map(|v| voices.push(v))
                    }
                    1 => h.stop_voice(voice),
                    2 => {
                        let gain = ((r >> 36) % 3) as f32 * 0.5;
                        h.set_voice_gain(voice, gain, ((r >> 40) % 3) as f64 * 0.05)
                    }
                    3 => h.set_voice_balance(voice, -0.3),
                    4 => h.set_voice_rate(voice, rate),
                    _ => Ok(()),
                };
                let frames = (r >> 48) as usize % 9;
                let buffer = &mut out[..frames * usize::from(channels)];
                m.process(buffer, &info);
                // `process` clamps the output, so check the voices themselves too.
                assert!(buffer.iter().all(|v| v.is_finite()));
                for v in &m.voices {
                    assert!((0.0..=1.0).contains(&v.gain), "gain {}", v.gain);
                    let end = (v.sample.frames() as u64) << 32;
                    assert!(!v.looping || v.pos < end);
                }
                if voices.len() > 32 {
                    voices.drain(..16);
                }
            }
            drop(m);
            h.collect();
            assert!(bank.iter().all(|s| Arc::strong_count(s) == 1));
        }
    }

    #[test]
    fn rejects_empty_config() {
        for config in [
            MixerConfig {
                max_voices: 0,
                ..Default::default()
            },
            MixerConfig {
                queue_capacity: 0,
                ..Default::default()
            },
            MixerConfig {
                channels: 0,
                ..Default::default()
            },
            MixerConfig {
                max_per_sample: 0,
                ..Default::default()
            },
            MixerConfig {
                queue_capacity: usize::MAX,
                ..Default::default()
            },
        ] {
            assert!(matches!(mixer(&config), Err(AudioError::InvalidConfig(_))));
        }
    }
}
