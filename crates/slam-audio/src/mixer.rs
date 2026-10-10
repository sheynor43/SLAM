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
use crate::sample::Sample;

/// Copies of one sample that sound at once in lazer: further plays replace the
/// longest-playing copy.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/OsuGameBase.cs (SAMPLE_CONCURRENCY),
// with osu-framework 2026.921.1: osu.Framework/Audio/Sample/SampleBassFactory.cs
// (BASS_SampleLoad max = concurrency, SampleOverrideLongestPlaying).
pub const SAMPLE_CONCURRENCY: usize = 6;

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
    Play { sample: Arc<Sample>, gain: f32 },
    StopAll,
    MasterGain(f32),
}

struct Voice {
    sample: Arc<Sample>,
    /// Next interleaved sample index in `sample.data()`.
    cursor: usize,
    gain: f32,
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
    stats: Arc<MixerStats>,
}

impl Mixer {
    fn apply_commands(&mut self, info: &CallbackInfo) {
        // Only the commands present now: a producer pushing during the callback cannot
        // stretch it.
        for _ in 0..self.commands.slots() {
            let Ok(command) = self.commands.pop() else {
                break;
            };
            match command {
                Command::Play { sample, gain } => {
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
                    self.voices.push(Voice {
                        sample,
                        cursor: 0,
                        gain,
                    });
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
        if info.channels != self.channels || info.sample_rate != self.sample_rate {
            self.stats.format_mismatches.fetch_add(1, Ordering::Relaxed);
            return;
        }

        let mut kept = 0;
        for i in 0..self.voices.len() {
            let voice = &mut self.voices[i];
            let data = voice.sample.data();
            let n = (data.len() - voice.cursor).min(out.len());
            let gain = voice.gain * self.master_gain;
            for (o, s) in out[..n]
                .iter_mut()
                .zip(&data[voice.cursor..voice.cursor + n])
            {
                *o += s * gain;
            }
            voice.cursor += n;
            if voice.cursor < data.len() {
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
        if sample.sample_rate() != self.sample_rate || sample.channels() != self.channels {
            return Err(PlayError::FormatMismatch);
        }
        if sample.data().is_empty() {
            // Nothing to hear; queuing it could only steal a sounding voice.
            return Ok(());
        }
        self.push(Command::Play {
            sample: Arc::clone(sample),
            gain: clamp_gain(gain),
        })?;
        self.queued_plays += 1;
        Ok(())
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

    fn push(&mut self, command: Command) -> Result<(), PlayError> {
        // Emptying the release ring before every push bounds the references the mixer
        // can hold; the ring capacity relies on it.
        self.collect();
        self.commands
            .push(command)
            .map_err(|rtrb::PushError::Full(_)| PlayError::QueueFull)
    }
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
