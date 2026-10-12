//! Music track streaming (ADR-0033): a decoder thread decodes and resamples the track
//! into fixed-size blocks, the audio callback plays them.
//!
//! [`music`] returns a [`MusicSource`] for the audio stream (usually attached to the
//! [`Mixer`](crate::Mixer)) and a [`MusicPlayer`] for the update thread, and starts the
//! decoder thread. Blocks travel through two SPSC rings (filled ones to the callback,
//! empty ones back), so the callback neither allocates nor frees. Every load and seek
//! starts a new generation; the callback drops blocks of older generations instead of
//! the decoder having to empty a ring it cannot read.
//!
//! Commands take effect at the first frame of the next audio buffer and are instant for
//! the track position, as in lazer. Fades (5 ms) only shape what is heard: a pause or
//! seek plays a copied tail of the old audio fading out without moving the track, and
//! audio fades in when it resumes.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Indexing, Resampler};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{TimeBase, Timestamp};

use crate::hal::{AudioCallback, AudioError, CallbackInfo};
use crate::mixer::PlayError;
use crate::sample::{MAX_CHANNELS, SAMPLE_RATES, SampleError, new_resampler, remix_into};
use crate::seqlock::SeqLock;

/// Length of every fade (pause and seek tails, fade-in, volume changes), milliseconds.
pub const MUSIC_FADE_MS: u32 = 5;

/// How long the decoder sleeps when it has nothing to fill (all blocks queued). The
/// callback cannot wake it (that is a system call), so it polls.
const DECODER_POLL: Duration = Duration::from_millis(5);

/// How long the decoder sleeps without a track; commands wake it at once.
const IDLE_POLL: Duration = Duration::from_millis(50);

/// Decoder events that can wait for [`MusicPlayer::poll_event`].
const EVENT_CAPACITY: usize = 16;

/// Music stream parameters. Rate and channels must match the output stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicConfig {
    /// Frames per second of the output stream.
    pub sample_rate: u32,
    /// Interleaved channels of the output stream.
    pub channels: u16,
    /// Frames per PCM block.
    pub block_frames: usize,
    /// Blocks in the pool: `blocks · block_frames` frames are decoded ahead.
    pub blocks: usize,
    /// Commands that can wait for the audio callback and for the decoder each.
    pub queue_capacity: usize,
}

impl Default for MusicConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            block_frames: 1_024,
            blocks: 16,
            queue_capacity: 32,
        }
    }
}

/// Where a track is read from.
#[derive(Debug, Clone)]
pub enum TrackSource {
    /// Streamed from a file; its extension hints the format.
    File(PathBuf),
    /// Held in memory (e.g. extracted from an archive); `extension` (without the dot)
    /// hints the format.
    Memory {
        bytes: Arc<[u8]>,
        extension: Option<Box<str>>,
    },
}

/// Why a track cannot be played.
#[derive(Debug, thiserror::Error)]
pub enum MusicError {
    #[error("cannot open track: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Format(#[from] SampleError),
    #[error("the music decoder thread stopped")]
    DecoderStopped,
}

/// Decoder reports, read with [`MusicPlayer::poll_event`].
#[derive(Debug)]
pub enum MusicEvent {
    /// The track of `generation` opened. `length` is in output frames when the
    /// container states it.
    Loaded {
        generation: u32,
        length: Option<u64>,
    },
    /// The track of `generation` failed to open or to decode; it plays as ended from
    /// where it stopped.
    Failed { generation: u32, error: MusicError },
}

/// Music counters, written by the audio and decoder threads (relaxed), read by anyone.
#[derive(Debug, Default)]
pub struct MusicStats {
    /// Buffers in which a playing track ran out of decoded audio.
    pub underruns: AtomicU64,
    /// Buffers skipped because the stream format differs from the music config.
    pub format_mismatches: AtomicU64,
    /// Blocks of an older generation dropped by the callback (after loads and seeks).
    pub discarded_blocks: AtomicU64,
    /// Blocks the decoder queued since creation.
    pub decoded_blocks: AtomicU64,
    /// Corrupt packets the decoder skipped.
    pub skipped_packets: AtomicU64,
    /// Decoder events dropped because nobody polled them.
    pub dropped_events: AtomicU64,
    /// Blocks leaked because a ring was full. Must stay 0: rings hold the whole pool.
    pub leaked_blocks: AtomicU64,
}

/// A stretch of the track timeline: from stream frame `device_frame` on, the track
/// advances `speed` track frames per stream frame from `track_frame`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MusicAnchor {
    /// Stream frame where the stretch starts, on the axis of [`CallbackInfo::frame`] and
    /// [`PositionSnapshot::frame`](crate::PositionSnapshot::frame).
    pub device_frame: u64,
    /// Track frame (at the stream rate) at `device_frame`.
    pub track_frame: u64,
    /// 1 while playing; 0 while paused, waiting for decoded audio or ended.
    pub speed: f64,
}

impl MusicAnchor {
    fn at(&self, device_frame: f64) -> f64 {
        self.track_frame as f64 + (device_frame - self.device_frame as f64) * self.speed
    }
}

/// Where the track is relative to the output stream, published after every buffer
/// (ADR-0033).
///
/// Anchors move only when the track changes course (play, pause, seek, underrun, end),
/// at the stream frame the callback was writing. The frame being heard lags behind by
/// the output latency, so the stretch before the latest change is kept too:
/// [`track_frame_at`](Self::track_frame_at) of the frame leaving the device is the track
/// frame being heard, continuous across changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MusicSnapshot {
    /// The stretch the callback is in now.
    pub anchor: MusicAnchor,
    /// The stretch before `anchor`.
    pub previous: MusicAnchor,
    /// Generation (load or seek) the callback is at; see [`MusicPlayer::generation`].
    /// It describes the frame being written: while the heard frame lies before the
    /// seek's anchor, [`track_frame_at`](Self::track_frame_at) still gives the old one.
    pub generation: u32,
    /// The callback reached the end of the track (or it failed).
    pub ended: bool,
}

impl MusicSnapshot {
    /// Track frame (fractional) at stream frame `device_frame`, e.g. the frame leaving
    /// the device from
    /// [`PositionSnapshot::output_frame_at`](crate::PositionSnapshot::output_frame_at).
    /// Before the previous anchor (two changes within the output latency) it is the
    /// previous anchor's track frame.
    pub fn track_frame_at(&self, device_frame: f64) -> f64 {
        if device_frame >= self.anchor.device_frame as f64 {
            self.anchor.at(device_frame)
        } else if device_frame >= self.previous.device_frame as f64 {
            self.previous.at(device_frame)
        } else {
            self.previous.track_frame as f64
        }
    }
}

/// Seqlock-protected [`MusicSnapshot`]. Writer: the audio callback. Readers: any thread.
#[derive(Debug, Default)]
pub struct MusicPosition {
    lock: SeqLock<7>,
}

impl MusicPosition {
    fn publish(&self, s: MusicSnapshot) {
        self.lock.write([
            s.anchor.device_frame,
            s.anchor.track_frame,
            s.anchor.speed.to_bits(),
            s.previous.device_frame,
            s.previous.track_frame,
            s.previous.speed.to_bits(),
            u64::from(s.generation) | u64::from(s.ended) << 32,
        ]);
    }

    /// Latest consistent snapshot, or `None` before the first buffer. Allocation-free and
    /// lock-free.
    pub fn read(&self) -> Option<MusicSnapshot> {
        self.lock.read().map(|w| MusicSnapshot {
            anchor: MusicAnchor {
                device_frame: w[0],
                track_frame: w[1],
                speed: f64::from_bits(w[2]),
            },
            previous: MusicAnchor {
                device_frame: w[3],
                track_frame: w[4],
                speed: f64::from_bits(w[5]),
            },
            generation: w[6] as u32,
            ended: w[6] >> 32 & 1 == 1,
        })
    }
}

/// PCM in the stream format, labelled with the generation and the track frame of its
/// first frame.
struct Block {
    data: Box<[f32]>,
    frames: usize,
    generation: u32,
    start: u64,
    /// No blocks of this generation follow.
    end: bool,
}

#[derive(Debug, Clone, Copy)]
enum AudioCommand {
    Play,
    /// Play from this stream frame on; at once if it has passed.
    PlayAt(u64),
    Pause,
    Volume(f32),
    /// Drop audio of older generations and stand at `frame` until blocks of
    /// `generation` arrive.
    Flush {
        generation: u32,
        frame: u64,
    },
}

enum DecoderCommand {
    Load {
        generation: u32,
        source: TrackSource,
    },
    Seek {
        generation: u32,
        frame: u64,
    },
    Unload {
        generation: u32,
    },
}

/// Creates the music source for the audio stream and its player for the update thread,
/// and starts the decoder thread `slam-audio-music`.
///
/// All memory the callback uses is allocated here.
pub fn music(config: &MusicConfig) -> Result<(MusicSource, MusicPlayer), AudioError> {
    let (source, mut player, decoder) = parts(config)?;
    let thread = std::thread::Builder::new()
        .name("slam-audio-music".into())
        .spawn(move || decoder.run())
        .map_err(|_| AudioError::Stream("cannot start the music decoder thread".into()))?;
    player.thread = Some(thread);
    Ok((source, player))
}

/// The three sides of a music stream; the decoder is not running yet.
fn parts(config: &MusicConfig) -> Result<(MusicSource, MusicPlayer, Decoder), AudioError> {
    if config.channels == 0
        || config.channels > MAX_CHANNELS
        || !SAMPLE_RATES.contains(&config.sample_rate)
    {
        return Err(AudioError::InvalidConfig(
            "music needs a rate in 1000..=384000 and 1..=8 channels",
        ));
    }
    if config.block_frames == 0 || config.blocks == 0 || config.queue_capacity < 2 {
        return Err(AudioError::InvalidConfig(
            "music needs blocks and room for two queued commands",
        ));
    }
    let channels = usize::from(config.channels);
    let block_values = config
        .block_frames
        .checked_mul(channels)
        .ok_or(AudioError::InvalidConfig("music block too large"))?;
    // Every block is in exactly one place (a ring, the callback, the decoder), so rings
    // as large as the pool never fill.
    let (filled_tx, filled_rx) = rtrb::RingBuffer::new(config.blocks);
    let (mut free_tx, free_rx) = rtrb::RingBuffer::new(config.blocks);
    for _ in 0..config.blocks {
        let block = Block {
            data: vec![0.0; block_values].into_boxed_slice(),
            frames: 0,
            generation: 0,
            start: 0,
            end: false,
        };
        if free_tx.push(block).is_err() {
            unreachable!("the free ring holds the whole pool");
        }
    }
    let (audio_tx, audio_rx) = rtrb::RingBuffer::new(config.queue_capacity);
    let (decoder_tx, decoder_rx) = rtrb::RingBuffer::new(config.queue_capacity);
    let (event_tx, event_rx) = rtrb::RingBuffer::new(EVENT_CAPACITY);
    let stats = Arc::new(MusicStats::default());
    let position = Arc::new(MusicPosition::default());
    let quit = Arc::new(AtomicBool::new(false));
    let fade_frames =
        (u64::from(config.sample_rate) * u64::from(MUSIC_FADE_MS) / 1_000).max(1) as usize;

    let decoder = Decoder {
        commands: decoder_rx,
        free: free_rx,
        filled: filled_tx,
        events: event_tx,
        quit: Arc::clone(&quit),
        stats: Arc::clone(&stats),
        sample_rate: config.sample_rate,
        channels: config.channels,
        generation: 0,
        stream: None,
        pending_end: false,
        block: None,
    };

    let source = MusicSource {
        commands: audio_rx,
        filled: filled_rx,
        free: free_tx,
        current: None,
        cursor: 0,
        generation: 0,
        track_frame: 0,
        playing: false,
        start_at: None,
        ended: false,
        primed: false,
        env: 0.0,
        gain: 1.0,
        target_gain: 1.0,
        step: 1.0 / fade_frames as f32,
        tail: vec![0.0; fade_frames * channels].into_boxed_slice(),
        tail_frames: 0,
        tail_pos: 0,
        tail_gain: 0.0,
        anchor: MusicAnchor::default(),
        previous: MusicAnchor::default(),
        sample_rate: config.sample_rate,
        channels: config.channels,
        position: Arc::clone(&position),
        stats: Arc::clone(&stats),
    };
    let player = MusicPlayer {
        audio: audio_tx,
        decoder: decoder_tx,
        events: event_rx,
        thread: None,
        quit,
        generation: 0,
        loaded: 0,
        length: None,
        stop_reported: false,
        sample_rate: config.sample_rate,
        position,
        stats,
    };
    Ok((source, player, decoder))
}

/// `a` is an older generation than `b` (wrapping).
fn is_older(a: u32, b: u32) -> bool {
    (b.wrapping_sub(a) as i32) > 0
}

enum Ready {
    /// The current block has frames of the current generation left.
    Data,
    /// No decoded audio of the current generation yet.
    Wait,
    /// The next block is of a newer generation, whose flush is still queued.
    Pending,
    /// The current generation played to its end.
    End,
}

/// Audio-thread side: mixes the track into the output buffer.
pub struct MusicSource {
    commands: rtrb::Consumer<AudioCommand>,
    filled: rtrb::Consumer<Block>,
    free: rtrb::Producer<Block>,
    current: Option<Block>,
    /// Next frame of `current`.
    cursor: usize,
    generation: u32,
    /// Track frame of the next frame to play.
    track_frame: u64,
    playing: bool,
    /// Stream frame a pending [`AudioCommand::PlayAt`] starts the track at.
    start_at: Option<u64>,
    ended: bool,
    /// A block of the current generation arrived: running dry now is an underrun.
    primed: bool,
    /// Fade-in envelope, 0..=1.
    env: f32,
    gain: f32,
    target_gain: f32,
    /// Envelope and gain change per frame: one fade over `MUSIC_FADE_MS`.
    step: f32,
    /// Audio copied at a pause or seek, fading out without moving the track.
    tail: Box<[f32]>,
    tail_frames: usize,
    tail_pos: usize,
    tail_gain: f32,
    anchor: MusicAnchor,
    previous: MusicAnchor,
    sample_rate: u32,
    channels: u16,
    position: Arc<MusicPosition>,
    stats: Arc<MusicStats>,
}

impl MusicSource {
    /// Adds the track to `out` (interleaved, the configured format) and publishes the
    /// position. Applies the commands queued before this buffer first. Does not clip.
    pub fn mix_into(&mut self, out: &mut [f32], info: &CallbackInfo) {
        self.apply_commands();
        if info.channels != self.channels || info.sample_rate != self.sample_rate {
            self.stats.format_mismatches.fetch_add(1, Ordering::Relaxed);
            self.mark(info.frame, 0.0);
            self.publish();
            return;
        }
        let ch = usize::from(self.channels);
        let frames = out.len() / ch;

        if self.tail_pos < self.tail_frames {
            let n = frames.min(self.tail_frames - self.tail_pos);
            let len = self.tail_frames as f32;
            for f in 0..n {
                let pos = self.tail_pos + f;
                let g = self.tail_gain * (1.0 - pos as f32 / len);
                for c in 0..ch {
                    out[f * ch + c] += self.tail[pos * ch + c] * g;
                }
            }
            self.tail_pos += n;
        }

        if self.env == 0.0 {
            // Nothing audible: a volume change needs no ramp.
            self.gain = self.target_gain;
        }
        if !self.playing {
            // A seek past the end or a failed track reads as ended while paused too.
            if matches!(self.ready(), Ready::End) {
                self.ended = true;
            }
            self.mark(info.frame, 0.0);
        }
        let mut i = 0;
        if let Some(at) = self.start_at
            && !self.playing
            && at < info.frame + frames as u64
        {
            // Scheduled start inside this buffer, or at once if the frame has passed.
            self.start_at = None;
            self.playing = true;
            i = at.saturating_sub(info.frame) as usize;
        }
        while self.playing && i < frames {
            match self.ready() {
                Ready::Data => {
                    self.mark(info.frame + i as u64, 1.0);
                    let Some(block) = &self.current else { break };
                    let n = (frames - i).min(block.frames - self.cursor);
                    if self.env == 1.0 && self.gain == self.target_gain {
                        // Steady state: one gain for the whole stretch (vectorizes).
                        let g = self.gain;
                        let src = &block.data[self.cursor * ch..(self.cursor + n) * ch];
                        for (o, s) in out[i * ch..(i + n) * ch].iter_mut().zip(src) {
                            *o += s * g;
                        }
                    } else {
                        for f in 0..n {
                            self.env = (self.env + self.step).min(1.0);
                            self.gain = if self.gain < self.target_gain {
                                (self.gain + self.step).min(self.target_gain)
                            } else {
                                (self.gain - self.step).max(self.target_gain)
                            };
                            let g = self.env * self.gain;
                            let src = (self.cursor + f) * ch;
                            let dst = (i + f) * ch;
                            for c in 0..ch {
                                out[dst + c] += block.data[src + c] * g;
                            }
                        }
                    }
                    self.cursor += n;
                    self.track_frame += n as u64;
                    i += n;
                }
                ready @ (Ready::Wait | Ready::Pending) => {
                    // Waiting for a generation whose flush is not applied yet is not
                    // the decoder's fault.
                    if self.primed && matches!(ready, Ready::Wait) {
                        self.stats.underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    self.env = 0.0;
                    self.mark(info.frame + i as u64, 0.0);
                    break;
                }
                Ready::End => {
                    self.ended = true;
                    self.env = 0.0;
                    self.mark(info.frame + i as u64, 0.0);
                    break;
                }
            }
        }
        if self.playing && i == frames && matches!(self.ready(), Ready::End) {
            // Ended exactly at the buffer end: report it now, not a buffer late.
            self.ended = true;
            self.env = 0.0;
            self.mark(info.frame + frames as u64, 0.0);
        }
        self.publish();
    }

    /// Stream rate and channels this source mixes.
    pub(crate) fn format(&self) -> (u32, u16) {
        (self.sample_rate, self.channels)
    }

    /// Starts a new anchor at stream frame `device_frame` unless the current one already
    /// describes the track there.
    fn mark(&mut self, device_frame: u64, speed: f64) {
        let a = self.anchor;
        // Exact for speeds 0 and 1 only; time-stretch (#86) needs a fractional check.
        let continues = a.speed == speed
            && device_frame >= a.device_frame
            && a.track_frame + (device_frame - a.device_frame) * speed as u64 == self.track_frame;
        if !continues {
            self.previous = a;
            self.anchor = MusicAnchor {
                device_frame,
                track_frame: self.track_frame,
                speed,
            };
        }
    }

    fn publish(&self) {
        self.position.publish(MusicSnapshot {
            anchor: self.anchor,
            previous: self.previous,
            generation: self.generation,
            ended: self.ended,
        });
    }

    fn apply_commands(&mut self) {
        // Only the commands present now: a producer pushing during the callback cannot
        // stretch it.
        for _ in 0..self.commands.slots() {
            let Ok(command) = self.commands.pop() else {
                break;
            };
            match command {
                AudioCommand::Play => {
                    self.playing = true;
                    self.start_at = None;
                }
                AudioCommand::PlayAt(frame) => {
                    if !self.playing {
                        self.start_at = Some(frame);
                    }
                }
                AudioCommand::Pause => {
                    self.start_at = None;
                    if self.playing {
                        self.cut_tail();
                        self.playing = false;
                    }
                }
                AudioCommand::Volume(gain) => self.target_gain = gain,
                AudioCommand::Flush { generation, frame } => {
                    if self.playing {
                        self.cut_tail();
                    }
                    if let Some(block) = self.current.take() {
                        self.recycle(block);
                    }
                    self.generation = generation;
                    self.track_frame = frame;
                    self.ended = false;
                    self.primed = false;
                }
            }
        }
    }

    /// Copies the next frames of what is sounding into the tail buffer, to fade out
    /// while the track stands still, and silences the main signal.
    fn cut_tail(&mut self) {
        if self.env == 0.0 {
            // Nothing sounding: keep a tail an earlier command started.
            return;
        }
        let ch = usize::from(self.channels);
        let capacity = self.tail.len() / ch;
        let mut copied = 0;
        if let Some(block) = &self.current
            && block.generation == self.generation
        {
            let n = capacity.min(block.frames - self.cursor);
            self.tail[..n * ch]
                .copy_from_slice(&block.data[self.cursor * ch..(self.cursor + n) * ch]);
            copied = n;
            if let Ok(next) = self.filled.peek()
                && next.generation == self.generation
            {
                let m = (capacity - copied).min(next.frames);
                self.tail[copied * ch..(copied + m) * ch].copy_from_slice(&next.data[..m * ch]);
                copied += m;
            }
        }
        self.tail_frames = copied;
        self.tail_pos = 0;
        self.tail_gain = self.env * self.gain;
        self.env = 0.0;
    }

    /// Makes `current` a block of the current generation with frames left, if any.
    fn ready(&mut self) -> Ready {
        loop {
            if let Some(block) = &self.current {
                if block.generation == self.generation {
                    if self.cursor < block.frames {
                        return Ready::Data;
                    }
                    if block.end {
                        // Kept: the track stays ended until the next flush.
                        return Ready::End;
                    }
                }
                if let Some(block) = self.current.take() {
                    self.recycle(block);
                }
            }
            match self.filled.peek() {
                Err(_) => return Ready::Wait,
                Ok(block) if is_older(block.generation, self.generation) => {
                    if let Ok(block) = self.filled.pop() {
                        self.stats.discarded_blocks.fetch_add(1, Ordering::Relaxed);
                        self.recycle(block);
                    }
                }
                // Newer: the flush for it is still in the command queue.
                Ok(block) if block.generation != self.generation => return Ready::Pending,
                Ok(block) => {
                    debug_assert!(
                        block.frames == 0 || block.start == self.track_frame,
                        "decoder and callback disagree on the track frame"
                    );
                    self.current = self.filled.pop().ok();
                    self.cursor = 0;
                    self.primed = true;
                }
            }
        }
    }

    fn recycle(&mut self, block: Block) {
        if let Err(rtrb::PushError::Full(block)) = self.free.push(block) {
            // Unreachable: the ring holds the whole pool. Leaking beats freeing on the
            // audio thread.
            self.stats.leaked_blocks.fetch_add(1, Ordering::Relaxed);
            std::mem::forget(block);
        }
    }
}

impl AudioCallback for MusicSource {
    fn process(&mut self, out: &mut [f32], info: &CallbackInfo) {
        out.fill(0.0);
        self.mix_into(out, info);
        for o in out.iter_mut() {
            *o = o.clamp(-1.0, 1.0);
        }
    }
}

impl std::fmt::Debug for MusicSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MusicSource")
            .field("generation", &self.generation)
            .field("track_frame", &self.track_frame)
            .field("playing", &self.playing)
            .finish_non_exhaustive()
    }
}

/// Update-thread side: controls playback. Commands never block; [`load`](Self::load)
/// takes the source by value, so building it (a path, shared bytes) is the caller's
/// allocation, outside the tick.
pub struct MusicPlayer {
    audio: rtrb::Producer<AudioCommand>,
    decoder: rtrb::Producer<DecoderCommand>,
    events: rtrb::Consumer<MusicEvent>,
    thread: Option<JoinHandle<()>>,
    quit: Arc<AtomicBool>,
    generation: u32,
    /// Generation of the last load: its `Loaded` event carries the length.
    loaded: u32,
    length: Option<u64>,
    /// The decoder thread's death was reported.
    stop_reported: bool,
    sample_rate: u32,
    position: Arc<MusicPosition>,
    stats: Arc<MusicStats>,
}

impl MusicPlayer {
    /// Starts loading `source`, paused at frame 0, and returns its generation. Playback
    /// state of the previous track does not carry over.
    pub fn load(&mut self, source: TrackSource) -> Result<u32, PlayError> {
        let generation = self.generation.wrapping_add(1);
        self.send(
            &[
                AudioCommand::Pause,
                AudioCommand::Flush {
                    generation,
                    frame: 0,
                },
            ],
            Some(DecoderCommand::Load { generation, source }),
        )?;
        self.generation = generation;
        self.loaded = generation;
        self.length = None;
        Ok(generation)
    }

    /// Stops and forgets the track.
    pub fn unload(&mut self) -> Result<(), PlayError> {
        let generation = self.generation.wrapping_add(1);
        self.send(
            &[
                AudioCommand::Pause,
                AudioCommand::Flush {
                    generation,
                    frame: 0,
                },
            ],
            Some(DecoderCommand::Unload { generation }),
        )?;
        self.generation = generation;
        // A late `Loaded` of the unloaded track must not bring its length back.
        self.loaded = generation;
        self.length = None;
        Ok(())
    }

    /// Plays from the current position from the next audio buffer.
    pub fn play(&mut self) -> Result<(), PlayError> {
        self.send(&[AudioCommand::Play], None)
    }

    /// Plays from the current position starting exactly at stream frame `device_frame`
    /// (the axis of [`CallbackInfo::frame`]), or from the next audio buffer if that frame
    /// has passed. Ignored while playing; [`pause`](Self::pause) and
    /// [`play`](Self::play) cancel it, a [`seek`](Self::seek) keeps it. Starting inside the
    /// buffer that applies a seek sets two anchors at once, so the stretch before the
    /// seek drops out of [`MusicSnapshot::previous`].
    pub fn play_at(&mut self, device_frame: u64) -> Result<(), PlayError> {
        self.send(&[AudioCommand::PlayAt(device_frame)], None)
    }

    /// Stops at the current position from the next audio buffer.
    pub fn pause(&mut self) -> Result<(), PlayError> {
        self.send(&[AudioCommand::Pause], None)
    }

    /// Moves to track frame `frame` (at the stream rate) from the next audio buffer,
    /// keeping play or pause. A frame at or past the known length ends the track.
    pub fn seek(&mut self, frame: u64) -> Result<(), PlayError> {
        let frame = self.length.map_or(frame, |length| frame.min(length));
        let generation = self.generation.wrapping_add(1);
        self.send(
            &[AudioCommand::Flush { generation, frame }],
            Some(DecoderCommand::Seek { generation, frame }),
        )?;
        self.generation = generation;
        Ok(())
    }

    /// Sets the track volume, linear, clamped to 0..=1 (non-finite counts as 0).
    pub fn set_volume(&mut self, gain: f32) -> Result<(), PlayError> {
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.send(&[AudioCommand::Volume(gain)], None)
    }

    /// Next decoder event, oldest first. Events of earlier generations are still
    /// returned; compare `generation`. Read it outside the update tick: dropping a
    /// [`MusicEvent::Failed`] frees its message.
    pub fn poll_event(&mut self) -> Option<MusicEvent> {
        let Ok(event) = self.events.pop() else {
            // A decoder that stopped (panicked, or its audio side is gone) sends
            // nothing; say so once.
            if !self.stop_reported && self.thread.as_ref().is_some_and(|t| t.is_finished()) {
                self.stop_reported = true;
                return Some(MusicEvent::Failed {
                    generation: self.generation,
                    error: MusicError::DecoderStopped,
                });
            }
            return None;
        };
        if let MusicEvent::Loaded { generation, length } = event
            && generation == self.loaded
        {
            self.length = length;
        }
        Some(event)
    }

    /// Generation of the last load or seek: positions with this generation reflect it.
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Track length in output frames, once [`MusicEvent::Loaded`] has stated it.
    pub fn length(&self) -> Option<u64> {
        self.length
    }

    /// Output frames per second: the unit of track frames.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Track position published by the audio callback.
    pub fn position(&self) -> &Arc<MusicPosition> {
        &self.position
    }

    /// Music counters.
    pub fn stats(&self) -> &Arc<MusicStats> {
        &self.stats
    }

    /// Queues `audio` and `decoder` together or not at all.
    fn send(
        &mut self,
        audio: &[AudioCommand],
        decoder: Option<DecoderCommand>,
    ) -> Result<(), PlayError> {
        if self.audio.slots() < audio.len() || (decoder.is_some() && self.decoder.slots() == 0) {
            return Err(PlayError::QueueFull);
        }
        for &command in audio {
            if self.audio.push(command).is_err() {
                unreachable!("slots were checked above");
            }
        }
        if let Some(command) = decoder {
            if self.decoder.push(command).is_err() {
                unreachable!("slots were checked above");
            }
            if let Some(thread) = &self.thread {
                thread.thread().unpark();
            }
        }
        Ok(())
    }
}

impl Drop for MusicPlayer {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            // A panicking decoder already reported itself; nothing to add here.
            let _ = thread.join();
        }
    }
}

impl std::fmt::Debug for MusicPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MusicPlayer")
            .field("generation", &self.generation)
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

/// The decoder thread: fills empty blocks with the current generation's audio.
struct Decoder {
    commands: rtrb::Consumer<DecoderCommand>,
    free: rtrb::Consumer<Block>,
    filled: rtrb::Producer<Block>,
    events: rtrb::Producer<MusicEvent>,
    quit: Arc<AtomicBool>,
    stats: Arc<MusicStats>,
    sample_rate: u32,
    channels: u16,
    generation: u32,
    stream: Option<Stream>,
    /// Queue an empty end block (the track failed to open).
    pending_end: bool,
    /// Block being filled.
    block: Option<Block>,
}

impl Decoder {
    fn run(mut self) {
        while !self.quit.load(Ordering::Relaxed) && !self.filled.is_abandoned() {
            while let Ok(command) = self.commands.pop() {
                self.handle(command);
            }
            if !self.fill() {
                let decoding = self.stream.as_ref().is_some_and(|s| !s.finished);
                std::thread::park_timeout(if decoding { DECODER_POLL } else { IDLE_POLL });
            }
        }
    }

    fn handle(&mut self, command: DecoderCommand) {
        match command {
            DecoderCommand::Load { generation, source } => {
                self.generation = generation;
                self.stream = None;
                self.pending_end = false;
                match Stream::open(source, self.sample_rate, self.channels) {
                    Ok(stream) => {
                        let length = stream.length;
                        self.stream = Some(stream);
                        self.event(MusicEvent::Loaded { generation, length });
                    }
                    Err(error) => {
                        self.pending_end = true;
                        self.event(MusicEvent::Failed { generation, error });
                    }
                }
            }
            DecoderCommand::Seek { generation, frame } => {
                self.generation = generation;
                match &mut self.stream {
                    Some(stream) => stream.seek(frame),
                    // Failed or unloaded: the new generation ends at once.
                    None => self.pending_end = true,
                }
            }
            DecoderCommand::Unload { generation } => {
                self.generation = generation;
                self.stream = None;
                self.pending_end = false;
            }
        }
    }

    fn event(&mut self, event: MusicEvent) {
        if self.events.push(event).is_err() {
            self.stats.dropped_events.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Fills and queues one block; false when there is nothing to do or no free block.
    fn fill(&mut self) -> bool {
        let active = self.stream.as_ref().is_some_and(|s| !s.finished) || self.pending_end;
        if !active {
            return false;
        }
        let Some(mut block) = self.block.take().or_else(|| self.free.pop().ok()) else {
            return false;
        };
        let ch = usize::from(self.channels);
        block.generation = self.generation;
        block.frames = 0;
        block.end = false;
        match &mut self.stream {
            Some(stream) if !self.pending_end => {
                block.start = stream.next_frame;
                let capacity = block.data.len() / ch;
                while block.frames < capacity {
                    match stream.read(&mut block.data[block.frames * ch..], &self.stats) {
                        Ok(0) => {
                            block.end = true;
                            break;
                        }
                        Ok(n) => block.frames += n,
                        Err(error) => {
                            block.end = true;
                            stream.finished = true;
                            let generation = self.generation;
                            self.event(MusicEvent::Failed { generation, error });
                            break;
                        }
                    }
                }
            }
            _ => {
                block.start = 0;
                block.end = true;
                self.pending_end = false;
            }
        }
        if let Err(rtrb::PushError::Full(block)) = self.filled.push(block) {
            // Unreachable: the ring holds the whole pool. Keep it for the next round.
            self.block = Some(block);
            return false;
        }
        self.stats.decoded_blocks.fetch_add(1, Ordering::Relaxed);
        true
    }
}

/// An open track: demuxer, decoder and resampler state.
struct Stream {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    src_rate: u32,
    /// Channels of the decoded buffers, fixed by the first one.
    src_channels: Option<u16>,
    channels: u16,
    out_rate: u32,
    length: Option<u64>,
    resampler: Option<Box<dyn Resampler<f32>>>,
    /// Decoded audio at the output channel count and the source rate.
    input: Vec<f32>,
    /// Resampled audio not yet copied into blocks; `out_pos` values already copied.
    output: Vec<f32>,
    out_pos: usize,
    packet: Vec<f32>,
    /// Source frames before this one are dropped after decoding (seek target).
    skip_until: i64,
    /// Output frames of resampler delay still to drop.
    skip_output: usize,
    /// Source frames given to the resampler and output frames kept since the last seek.
    fed: u64,
    emitted: u64,
    eof: bool,
    /// Everything up to the end has been handed out.
    finished: bool,
    /// Track frame of the next output frame.
    next_frame: u64,
    /// Track frame the pipeline restarted at.
    start_frame: u64,
}

impl Stream {
    fn open(source: TrackSource, out_rate: u32, channels: u16) -> Result<Self, MusicError> {
        let decode_err = |e: SymphoniaError| SampleError::Decode(e.to_string());
        let mut hint = Hint::new();
        let media: Box<dyn MediaSource> = match source {
            TrackSource::File(path) => {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    hint.with_extension(ext);
                }
                Box::new(std::fs::File::open(path)?)
            }
            TrackSource::Memory { bytes, extension } => {
                if let Some(ext) = &extension {
                    hint.with_extension(ext);
                }
                Box::new(Cursor::new(bytes))
            }
        };
        let source = MediaSourceStream::new(media, Default::default());
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                source,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(decode_err)?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or(SampleError::NoTrack)?;
        let track_id = track.id;
        let time_base = track.time_base;
        let num_frames = track.num_frames;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or(SampleError::NoTrack)?;
        let src_rate = params
            .sample_rate
            .ok_or(SampleError::InvalidFormat("unknown sample rate"))?;
        if !SAMPLE_RATES.contains(&src_rate) {
            return Err(SampleError::InvalidFormat("sample rate out of 1000..=384000").into());
        }
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(decode_err)?;
        let resampler = if src_rate == out_rate {
            None
        } else {
            Some(new_resampler(src_rate, out_rate, usize::from(channels))?)
        };
        let length = num_frames.map(|n| scale_up(n, src_rate, out_rate));
        let mut stream = Self {
            format,
            decoder,
            track_id,
            time_base,
            src_rate,
            src_channels: None,
            channels,
            out_rate,
            length,
            resampler,
            input: Vec::new(),
            output: Vec::new(),
            out_pos: 0,
            packet: Vec::new(),
            skip_until: 0,
            skip_output: 0,
            fed: 0,
            emitted: 0,
            eof: false,
            finished: false,
            next_frame: 0,
            start_frame: 0,
        };
        stream.restart(0, 0);
        Ok(stream)
    }

    /// Resets the pipeline to output track frame `frame`, source frame `src_frame`.
    fn restart(&mut self, frame: u64, src_frame: i64) {
        self.input.clear();
        self.output.clear();
        self.out_pos = 0;
        self.fed = 0;
        self.emitted = 0;
        self.eof = false;
        self.finished = false;
        self.skip_until = src_frame;
        self.next_frame = frame;
        self.start_frame = frame;
        self.skip_output = match &mut self.resampler {
            Some(r) => {
                r.reset();
                // Output frames to drop so that the first kept one is at `frame`: the
                // resampler delay, minus the half source frame by which `rubato`'s
                // output (FFT and sinc alike) runs ahead of `output_delay`, plus how far
                // `src_frame` (rounded down) starts before `frame`. In units of
                // 1 / (2·src) output frames, then rounded: at most ½ frame off.
                let (src, out) = (u128::from(self.src_rate), u128::from(self.out_rate));
                let twice = (2 * src * r.output_delay() as u128 + 2 * u128::from(frame) * src)
                    .saturating_sub(out + 2 * src_frame.max(0) as u128 * out);
                ((twice + src) / (2 * src)) as usize
            }
            None => 0,
        };
    }

    /// Restarts at output track frame `frame`.
    fn seek(&mut self, frame: u64) {
        // Rounded down; `restart` drops the output frames before `frame` (≤ ½ frame off).
        let src_frame = (u128::from(frame) * u128::from(self.src_rate) / u128::from(self.out_rate))
            .min(i64::MAX as u128) as i64;
        self.restart(frame, src_frame);
        if self.length.is_some_and(|length| frame >= length) {
            self.eof = true;
            return;
        }
        let ts = match self.time_base {
            // ts = frame / rate / (numer / denom), rounded down.
            Some(tb) => {
                let ts = i128::from(src_frame) * i128::from(tb.denom.get())
                    / (i128::from(tb.numer.get()) * i128::from(self.src_rate));
                ts.min(i128::from(i64::MAX)) as i64
            }
            None => src_frame,
        };
        let to = SeekTo::Timestamp {
            ts: Timestamp::new(ts),
            track_id: self.track_id,
        };
        match self.format.seek(SeekMode::Accurate, to) {
            Ok(_) => self.decoder.reset(),
            // Out of range or unseekable: nothing more to play from here.
            Err(_) => self.eof = true,
        }
    }

    /// Source frame of timestamp `ts`.
    fn frame_of(&self, ts: Timestamp) -> i64 {
        match self.time_base {
            Some(tb) => {
                let f =
                    i128::from(ts.get()) * i128::from(tb.numer.get()) * i128::from(self.src_rate)
                        / i128::from(tb.denom.get());
                f.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
            }
            None => ts.get(),
        }
    }

    /// Copies up to `out.len()` values of output audio; 0 only at the end.
    fn read(&mut self, out: &mut [f32], stats: &MusicStats) -> Result<usize, MusicError> {
        let ch = usize::from(self.channels);
        loop {
            let left = self.output.len() - self.out_pos;
            if left > 0 {
                let n = left.min(out.len()) / ch * ch;
                out[..n].copy_from_slice(&self.output[self.out_pos..self.out_pos + n]);
                self.out_pos += n;
                self.next_frame += (n / ch) as u64;
                return Ok(n / ch);
            }
            if self.finished {
                return Ok(0);
            }
            self.output.clear();
            self.out_pos = 0;
            self.produce(stats)?;
        }
    }

    /// Puts more audio into `output`, or sets `finished`.
    fn produce(&mut self, stats: &MusicStats) -> Result<(), MusicError> {
        let ch = usize::from(self.channels);
        let Some(resampler) = &mut self.resampler else {
            while self.input.is_empty() && !self.eof {
                self.decode_packet(stats)?;
            }
            if self.input.is_empty() {
                self.finished = true;
            }
            std::mem::swap(&mut self.input, &mut self.output);
            return Ok(());
        };
        let need = resampler.input_frames_next();
        while self.input.len() / ch < need && !self.eof {
            self.decode_packet(stats)?;
        }
        let Some(resampler) = &mut self.resampler else {
            unreachable!("checked above");
        };
        let need = resampler.input_frames_next();
        let available = self.input.len() / ch;
        let expected = scale_up(self.fed, self.src_rate, self.out_rate);
        let partial = available < need;
        if partial && self.emitted >= expected {
            self.finished = true;
            return Ok(());
        }
        let valid = available.min(need);
        if partial {
            // The resampler reads a whole chunk; the rest is silence.
            self.input.resize(need * ch, 0.0);
        }
        let resample_err = |e: &dyn std::fmt::Display| SampleError::Resample(e.to_string());
        let out_max = resampler.output_frames_max();
        self.output.resize(out_max * ch, 0.0);
        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: partial.then_some(valid),
            active_channels_mask: None,
        };
        let input = InterleavedSlice::new(&self.input[..need * ch], ch, need)
            .map_err(|e| resample_err(&e))?;
        let mut output = InterleavedSlice::new_mut(&mut self.output, ch, out_max)
            .map_err(|e| resample_err(&e))?;
        let (_, produced) = resampler
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .map_err(|e| resample_err(&e))?;
        self.input.drain(..valid * ch);
        if partial {
            self.input.clear();
        }
        self.fed += valid as u64;
        self.output.truncate(produced * ch);
        let delay = self.skip_output.min(produced);
        self.output.drain(..delay * ch);
        self.skip_output -= delay;
        if self.eof {
            // The exact length after the end is `ceil(fed · out / src)`. After a seek,
            // rounding could overshoot the stated length by a frame: cap it there (from
            // the start the computed length stands, even if the container's is off).
            let mut room =
                scale_up(self.fed, self.src_rate, self.out_rate).saturating_sub(self.emitted);
            if let Some(length) = self.length.filter(|_| self.start_frame > 0) {
                room = room.min(length.saturating_sub(self.start_frame + self.emitted));
            }
            let keep = (self.output.len() / ch).min(room as usize);
            self.output.truncate(keep * ch);
        }
        self.emitted += (self.output.len() / ch) as u64;
        Ok(())
    }

    /// Decodes the next packet into `input`, dropping frames before `skip_until`.
    fn decode_packet(&mut self, stats: &MusicStats) -> Result<(), MusicError> {
        let decode_err = |e: SymphoniaError| SampleError::Decode(e.to_string());
        let packet = match self.format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => {
                self.eof = true;
                return Ok(());
            }
            Err(SymphoniaError::ResetRequired) => {
                return Err(SampleError::InvalidFormat("format changes mid-stream").into());
            }
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                self.eof = true;
                return Ok(());
            }
            Err(e) => return Err(decode_err(e).into()),
        };
        if packet.track_id != self.track_id {
            return Ok(());
        }
        // `pts` counts the frames the decoder trims (encoder delay) at the start.
        let first = self.frame_of(packet.pts.saturating_add(packet.trim_start));
        let buffer = match self.decoder.decode(&packet) {
            Ok(buffer) => buffer,
            Err(SymphoniaError::DecodeError(_) | SymphoniaError::IoError(_)) => {
                stats.skipped_packets.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            Err(e) => return Err(decode_err(e).into()),
        };
        if buffer.spec().rate() != self.src_rate {
            return Err(SampleError::InvalidFormat("format changes mid-stream").into());
        }
        let channels = u16::try_from(buffer.spec().channels().count())
            .ok()
            .filter(|c| (1..=MAX_CHANNELS).contains(c))
            .ok_or(SampleError::TooLong)?;
        match self.src_channels {
            None => self.src_channels = Some(channels),
            Some(c) if c != channels => {
                return Err(SampleError::InvalidFormat("format changes mid-stream").into());
            }
            Some(_) => {}
        }
        buffer.copy_to_vec_interleaved::<f32>(&mut self.packet);
        let src_ch = usize::from(channels);
        let frames = self.packet.len() / src_ch;
        let skip = (self.skip_until.saturating_sub(first)).clamp(0, frames as i64) as usize;
        let start = self.input.len();
        remix_into(
            &self.packet[skip * src_ch..frames * src_ch],
            channels,
            self.channels,
            &mut self.input,
        );
        for v in &mut self.input[start..] {
            if !v.is_normal() {
                *v = 0.0;
            }
        }
        Ok(())
    }
}

/// `ceil(frames · to / from)`: a length converted between rates.
fn scale_up(frames: u64, from: u32, to: u32) -> u64 {
    (u128::from(frames) * u128::from(to))
        .div_ceil(u128::from(from))
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    /// Fade length at `RATE`.
    const FADE: usize = 240;
    /// Ramp values are `frame / RAMP_SCALE`: exact in `f32` for frames under 2^20.
    const RAMP_SCALE: f32 = (1 << 20) as f32;

    fn ramp(frame: usize) -> f32 {
        frame as f32 / RAMP_SCALE
    }

    /// A mono 32-bit float WAV.
    fn wav(rate: u32, data: &[f32]) -> TrackSource {
        let mut b = Vec::new();
        let bytes = (data.len() * 4) as u32;
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + bytes).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        b.extend_from_slice(&1u16.to_le_bytes()); // mono
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 4).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&bytes.to_le_bytes());
        for v in data {
            b.extend_from_slice(&v.to_le_bytes());
        }
        TrackSource::Memory {
            bytes: b.into(),
            extension: Some("wav".into()),
        }
    }

    fn ramp_track(frames: usize) -> TrackSource {
        wav(RATE, &(0..frames).map(ramp).collect::<Vec<_>>())
    }

    struct Rig {
        source: MusicSource,
        player: MusicPlayer,
        decoder: Decoder,
        frame: u64,
    }

    impl Rig {
        fn new(block_frames: usize, blocks: usize) -> Self {
            let (source, player, decoder) = parts(&MusicConfig {
                sample_rate: RATE,
                channels: 2,
                block_frames,
                blocks,
                queue_capacity: 8,
            })
            .unwrap();
            Self {
                source,
                player,
                decoder,
                frame: 0,
            }
        }

        /// What the decoder thread does until it runs out of free blocks.
        fn pump(&mut self) {
            loop {
                while let Ok(command) = self.decoder.commands.pop() {
                    self.decoder.handle(command);
                }
                if !self.decoder.fill() {
                    break;
                }
            }
        }

        /// One audio buffer of `frames` frames; returns the left channel.
        fn run(&mut self, frames: usize) -> Vec<f32> {
            let mut out = vec![9.0; frames * 2];
            let info = CallbackInfo {
                frame: self.frame,
                timestamp_ns: 0,
                latency_ns: 0,
                sample_rate: RATE,
                channels: 2,
            };
            self.source.process(&mut out, &info);
            self.frame += frames as u64;
            for frame in out.as_chunks::<2>().0 {
                assert_eq!(frame[0], frame[1], "mono must reach both channels");
            }
            out.iter().step_by(2).copied().collect()
        }

        fn snapshot(&self) -> MusicSnapshot {
            self.player.position().read().unwrap()
        }

        fn loaded(&mut self) -> Option<u64> {
            match self.player.poll_event() {
                Some(MusicEvent::Loaded { length, .. }) => length,
                other => panic!("expected Loaded, got {other:?}"),
            }
        }
    }

    /// Fade-in envelope after `f` frames of audio.
    fn env(f: usize) -> f32 {
        let mut e = 0.0f32;
        for _ in 0..=f {
            e = (e + 1.0 / FADE as f32).min(1.0);
        }
        e
    }

    #[test]
    fn plays_from_the_start_with_a_fade_in() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(20_000)).unwrap();
        rig.pump();
        assert_eq!(rig.loaded(), Some(20_000));
        // Paused after loading: silent, standing at 0.
        assert_eq!(rig.run(64), [0.0; 64]);
        assert_eq!(
            (
                rig.snapshot().anchor.track_frame,
                rig.snapshot().anchor.speed
            ),
            (0, 0.0)
        );
        rig.player.play().unwrap();
        let out = rig.run(1_000);
        for (f, &v) in out.iter().enumerate() {
            assert_eq!(v, ramp(f) * env(f), "frame {f}");
        }
        assert_eq!(out[FADE..], (FADE..1_000).map(ramp).collect::<Vec<_>>());
        let s = rig.snapshot();
        assert_eq!(
            (s.anchor.device_frame, s.anchor.track_frame, s.anchor.speed),
            (64, 0, 1.0)
        );
        assert_eq!(s.track_frame_at(64.0 + 1_000.0), 1_000.0);
        assert!(!s.ended);
    }

    #[test]
    fn seek_is_frame_exact_and_fades_the_old_audio_out() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(100_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.player.play().unwrap();
        rig.run(1_024);
        rig.player.seek(50_000).unwrap();
        // The decoder has not taken the seek yet: the ring holds only old audio, which
        // the callback drops.
        let out = rig.run(512);
        let s = rig.snapshot();
        assert_eq!(s.generation, rig.player.generation());
        assert_eq!((s.anchor.track_frame, s.anchor.speed), (50_000, 0.0));
        assert!(rig.source.stats.discarded_blocks.load(Ordering::Relaxed) > 0);
        // The tail continues the old audio from frame 1024, fading out.
        for (f, &v) in out[..FADE].iter().enumerate() {
            let want = ramp(1_024 + f) * (1.0 - f as f32 / FADE as f32);
            assert_eq!(v, want, "tail frame {f}");
        }
        assert_eq!(out[FADE..], [0.0; 512 - FADE]);
        // Waiting for the new generation is not an underrun.
        assert_eq!(rig.source.stats.underruns.load(Ordering::Relaxed), 0);

        rig.pump();
        let out = rig.run(512);
        for (f, &v) in out.iter().enumerate() {
            assert_eq!(v, ramp(50_000 + f) * env(f), "frame {f}");
        }
        let s = rig.snapshot();
        assert_eq!(
            (s.anchor.device_frame, s.anchor.track_frame, s.anchor.speed),
            (1_536, 50_000, 1.0)
        );
    }

    #[test]
    fn seek_with_decoded_audio_ready_crossfades_in_one_buffer() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(100_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.player.play().unwrap();
        rig.run(1_024);
        rig.player.seek(30_000).unwrap();
        // Leave the decoder blocks to fill right after the old ones are dropped.
        rig.run(0);
        rig.pump();
        let out = rig.run(512);
        for (f, &v) in out.iter().enumerate() {
            let tail = if f < FADE {
                ramp(1_024 + f) * (1.0 - f as f32 / FADE as f32)
            } else {
                0.0
            };
            let want = tail + ramp(30_000 + f) * env(f);
            assert!((v - want).abs() <= 1e-7, "frame {f}: {v} vs {want}");
        }
    }

    #[test]
    fn pause_stops_the_track_at_once_and_resumes_there() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(20_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.player.play().unwrap();
        rig.run(512);
        rig.player.pause().unwrap();
        let out = rig.run(512);
        let s = rig.snapshot();
        assert_eq!(
            (s.anchor.device_frame, s.anchor.track_frame, s.anchor.speed),
            (512, 512, 0.0)
        );
        for (f, &v) in out[..FADE].iter().enumerate() {
            assert_eq!(v, ramp(512 + f) * (1.0 - f as f32 / FADE as f32));
        }
        assert_eq!(out[FADE..], [0.0; 512 - FADE]);
        rig.player.play().unwrap();
        let out = rig.run(512);
        for (f, &v) in out.iter().enumerate() {
            assert_eq!(v, ramp(512 + f) * env(f), "frame {f}");
        }
        assert_eq!(rig.snapshot().anchor.track_frame, 512);
    }

    #[test]
    fn play_at_starts_the_track_on_that_stream_frame() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(20_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.player.play_at(700).unwrap();
        // Not yet: the buffer ends before frame 700.
        assert_eq!(rig.run(512), [0.0; 512]);
        assert_eq!(rig.snapshot().anchor.speed, 0.0);
        let out = rig.run(512);
        assert_eq!(out[..700 - 512], [0.0; 700 - 512]);
        for (f, &v) in out[700 - 512..].iter().enumerate() {
            assert_eq!(v, ramp(f) * env(f), "frame {f}");
        }
        let s = rig.snapshot();
        assert_eq!(
            (s.anchor.device_frame, s.anchor.track_frame, s.anchor.speed),
            (700, 0, 1.0)
        );
        assert_eq!(s.track_frame_at(1_000.0), 300.0);
        assert_eq!(s.track_frame_at(600.0), 0.0);
    }

    #[test]
    fn play_at_a_passed_frame_starts_with_the_next_buffer() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(20_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.run(512);
        rig.player.play_at(100).unwrap();
        rig.run(512);
        let s = rig.snapshot();
        assert_eq!(
            (s.anchor.device_frame, s.anchor.track_frame, s.anchor.speed),
            (512, 0, 1.0)
        );
    }

    #[test]
    fn play_at_before_the_first_block_is_not_an_underrun() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(20_000)).unwrap();
        rig.player.play_at(100).unwrap();
        rig.run(512);
        assert_eq!(rig.player.stats().underruns.load(Ordering::Relaxed), 0);
        rig.pump();
        rig.loaded();
        let out = rig.run(512);
        assert_eq!(out[0], ramp(0) * env(0));
        assert_eq!(rig.player.stats().underruns.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn pause_and_play_cancel_play_at() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(20_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.player.play_at(600).unwrap();
        rig.player.pause().unwrap();
        rig.run(1_024);
        assert_eq!(rig.snapshot().anchor.speed, 0.0);

        rig.player.play_at(2_000).unwrap();
        rig.player.play().unwrap();
        rig.run(512);
        let s = rig.snapshot();
        assert_eq!((s.anchor.device_frame, s.anchor.speed), (1_024, 1.0));
        // Ignored while playing.
        rig.player.play_at(5_000).unwrap();
        rig.run(512);
        assert_eq!(rig.snapshot().anchor.device_frame, 1_024);
    }

    #[test]
    fn running_dry_is_an_underrun_and_stops_the_track() {
        let mut rig = Rig::new(256, 2);
        rig.player.load(ramp_track(20_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.player.play().unwrap();
        rig.run(128);
        // Two blocks (512 frames) were decoded and the decoder is not run again.
        let out = rig.run(1_024);
        assert_eq!(out[384..], [0.0; 640]);
        assert_eq!(rig.source.stats.underruns.load(Ordering::Relaxed), 1);
        let s = rig.snapshot();
        assert_eq!(
            (s.anchor.device_frame, s.anchor.track_frame, s.anchor.speed),
            (512, 512, 0.0)
        );
        // The audio comes back where it stopped, fading in.
        rig.pump();
        let out = rig.run(256);
        for (f, &v) in out.iter().enumerate() {
            assert_eq!(v, ramp(512 + f) * env(f), "frame {f}");
        }
    }

    #[test]
    fn track_ends_and_seeks_past_the_end_clamp() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(1_000)).unwrap();
        rig.pump();
        assert_eq!(rig.loaded(), Some(1_000));
        rig.player.play().unwrap();
        let out = rig.run(2_048);
        assert_eq!(out[1_000..], [0.0; 1_048]);
        assert_eq!(out[999], ramp(999));
        let s = rig.snapshot();
        assert_eq!(
            (s.anchor.device_frame, s.anchor.track_frame, s.anchor.speed),
            (1_000, 1_000, 0.0)
        );
        assert!(s.ended);
        assert_eq!(rig.source.stats.underruns.load(Ordering::Relaxed), 0);

        rig.player.seek(5_000).unwrap();
        rig.pump();
        rig.run(256);
        let s = rig.snapshot();
        assert_eq!(
            (s.anchor.track_frame, s.anchor.speed, s.ended),
            (1_000, 0.0, true)
        );

        // Seeking back plays again.
        rig.player.seek(100).unwrap();
        rig.pump();
        let out = rig.run(512);
        assert_eq!(out[FADE..], (100 + FADE..612).map(ramp).collect::<Vec<_>>());
        assert!(!rig.snapshot().ended);
    }

    #[test]
    fn end_at_a_buffer_boundary_is_reported_in_that_buffer() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(1_024)).unwrap();
        rig.pump();
        rig.loaded();
        rig.player.play().unwrap();
        rig.run(1_024);
        let s = rig.snapshot();
        assert_eq!(
            (s.anchor.device_frame, s.anchor.track_frame, s.anchor.speed),
            (1_024, 1_024, 0.0)
        );
        assert!(s.ended);
    }

    #[test]
    fn volume_ramps_to_its_target() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(20_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.player.play().unwrap();
        rig.run(512);
        rig.player.set_volume(0.5).unwrap();
        let out = rig.run(512);
        assert!(out[0] > ramp(512) * 0.99, "no jump: {}", out[0]);
        assert_eq!(
            out[FADE..],
            (512 + FADE..1_024)
                .map(|f| ramp(f) * 0.5)
                .collect::<Vec<_>>()
        );
        // While paused a new volume applies at once.
        rig.player.pause().unwrap();
        rig.player.set_volume(0.25).unwrap();
        rig.run(512);
        rig.player.play().unwrap();
        let out = rig.run(512);
        assert_eq!(out[FADE], ramp(1_024 + FADE) * 0.25);
    }

    #[test]
    fn resampled_track_keeps_length_pitch_and_seek_alignment() {
        let src_rate = 44_100;
        let frames = 3 * src_rate as usize;
        let sine = |rate: u32, frame: f64| {
            ((frame / f64::from(rate) * 1_000.0) * std::f64::consts::TAU).sin() as f32
        };
        let data: Vec<f32> = (0..frames).map(|n| sine(src_rate, n as f64)).collect();
        let mut rig = Rig::new(1_024, 8);
        rig.player.load(wav(src_rate, &data)).unwrap();
        rig.pump();
        assert_eq!(rig.loaded(), Some(3 * u64::from(RATE)));
        rig.player.seek(u64::from(RATE)).unwrap();
        rig.player.play().unwrap();
        // The callback drops the blocks decoded from 0, then the decoder refills.
        rig.run(0);
        rig.pump();
        let mut out = Vec::new();
        for _ in 0..8 {
            out.extend(rig.run(1_024));
            rig.pump();
        }
        // After the fade the output follows the source sine on the 48 kHz grid,
        // offset by under one frame.
        let error_at = |lag: f64| {
            out.iter()
                .enumerate()
                .skip(FADE + 64)
                .map(|(k, &v)| (v - sine(RATE, (u64::from(RATE) + k as u64) as f64 - lag)).abs())
                .fold(0.0f32, f32::max)
        };
        let (error, lag) = (-30..=30)
            .map(|i| {
                let lag = f64::from(i) / 20.0;
                (error_at(lag), lag)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap();
        assert!(error < 0.01, "max error {error} at lag {lag}");
        assert!(lag.abs() <= 1.0, "lag {lag}");

        // Played to the end, the track is exactly `ceil(frames · 48000 / 44100)` long.
        for _ in 0..200 {
            rig.run(1_024);
            rig.pump();
        }
        let s = rig.snapshot();
        assert!(s.ended);
        assert_eq!(s.anchor.track_frame, 3 * u64::from(RATE));
    }

    #[test]
    fn unreadable_track_fails_and_plays_as_ended() {
        let mut rig = Rig::new(256, 4);
        rig.player
            .load(TrackSource::Memory {
                bytes: vec![0x5au8; 4_096].into(),
                extension: None,
            })
            .unwrap();
        rig.pump();
        assert!(matches!(
            rig.player.poll_event(),
            Some(MusicEvent::Failed { generation: 1, .. })
        ));
        rig.player.play().unwrap();
        assert_eq!(rig.run(128), [0.0; 128]);
        assert!(rig.snapshot().ended);
        assert_eq!(rig.source.stats.underruns.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn unload_silences_and_a_new_load_starts_paused() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(20_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.player.play().unwrap();
        rig.run(512);
        rig.player.unload().unwrap();
        rig.pump();
        let out = rig.run(512);
        assert_eq!(out[FADE..], [0.0; 512 - FADE]);
        assert_eq!(rig.player.length(), None);
        rig.player.load(ramp_track(5_000)).unwrap();
        rig.pump();
        assert_eq!(rig.loaded(), Some(5_000));
        assert_eq!(rig.run(256), [0.0; 256]);
        rig.player.play().unwrap();
        assert_eq!(rig.run(512)[FADE], ramp(FADE));
    }

    #[test]
    fn commands_go_to_both_queues_or_neither() {
        let mut rig = Rig::new(256, 8);
        // Fill the audio queue (capacity 8) with plays; a seek cannot be half-sent.
        while rig.player.play().is_ok() {}
        let generation = rig.player.generation();
        assert_eq!(rig.player.seek(10), Err(PlayError::QueueFull));
        assert_eq!(rig.player.generation(), generation);
        assert!(rig.decoder.commands.is_empty());
    }

    #[test]
    fn generations_compare_across_wraparound() {
        assert!(is_older(1, 2));
        assert!(!is_older(2, 2));
        assert!(!is_older(3, 2));
        assert!(is_older(u32::MAX, 0));
        assert!(!is_older(0, u32::MAX));
    }

    #[test]
    fn stream_format_mismatch_is_counted_and_silent() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(20_000)).unwrap();
        rig.pump();
        rig.player.play().unwrap();
        let mut out = [9.0; 64];
        rig.source.process(
            &mut out,
            &CallbackInfo {
                frame: 0,
                timestamp_ns: 0,
                latency_ns: 0,
                sample_rate: 44_100,
                channels: 2,
            },
        );
        assert_eq!(out, [0.0; 64]);
        assert_eq!(
            rig.source.stats.format_mismatches.load(Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn rejects_unusable_config() {
        for config in [
            MusicConfig {
                channels: 0,
                ..Default::default()
            },
            MusicConfig {
                sample_rate: 10,
                ..Default::default()
            },
            MusicConfig {
                blocks: 0,
                ..Default::default()
            },
            MusicConfig {
                block_frames: usize::MAX,
                ..Default::default()
            },
            MusicConfig {
                queue_capacity: 1,
                ..Default::default()
            },
        ] {
            assert!(matches!(parts(&config), Err(AudioError::InvalidConfig(_))));
        }
    }

    #[test]
    fn heard_position_is_continuous_across_changes_with_output_latency() {
        // The frame leaving the device lags the written one; the previous stretch
        // covers it.
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(100_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.run(128);
        rig.player.play().unwrap();
        rig.run(1_024);
        // Play started at stream frame 128.
        let s = rig.snapshot();
        assert_eq!(s.track_frame_at(100.0), 0.0);
        assert_eq!(s.track_frame_at(1_000.0), 872.0);

        rig.player.pause().unwrap();
        rig.run(512);
        // Paused at stream frame 1152 = track 1024; frames written before still play.
        let s = rig.snapshot();
        assert_eq!(s.track_frame_at(1_100.0), 972.0);
        assert_eq!(s.track_frame_at(1_152.0), 1_024.0);
        assert_eq!(s.track_frame_at(1_600.0), 1_024.0);

        rig.player.seek(40_000).unwrap();
        rig.player.play().unwrap();
        rig.run(0);
        rig.pump();
        rig.run(512);
        // Resumed at 1664 from the seek target; before it the pause still holds.
        let s = rig.snapshot();
        assert_eq!(s.track_frame_at(1_600.0), 1_024.0);
        assert_eq!(s.track_frame_at(1_664.0), 40_000.0);
        assert_eq!(s.track_frame_at(2_000.0), 40_336.0);
        // Older than both stretches: the previous anchor's frame.
        assert_eq!(s.track_frame_at(10.0), 1_024.0);
    }

    #[test]
    fn length_survives_a_seek_before_the_loaded_event() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(1_000)).unwrap();
        rig.player.seek(200).unwrap();
        rig.pump();
        assert_eq!(rig.loaded(), Some(1_000));
        assert_eq!(rig.player.length(), Some(1_000));
        // So later seeks clamp.
        rig.player.seek(9_999).unwrap();
        rig.pump();
        rig.run(64);
        let s = rig.snapshot();
        assert_eq!((s.anchor.track_frame, s.ended), (1_000, true));
    }

    #[test]
    fn late_loaded_event_of_an_unloaded_track_sets_no_length() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(1_000)).unwrap();
        rig.player.unload().unwrap();
        rig.pump();
        assert_eq!(rig.loaded(), Some(1_000));
        assert_eq!(rig.player.length(), None);
    }

    #[test]
    fn seek_after_a_failed_load_still_ends() {
        let mut rig = Rig::new(256, 4);
        rig.player
            .load(TrackSource::Memory {
                bytes: vec![0x5au8; 4_096].into(),
                extension: None,
            })
            .unwrap();
        rig.pump();
        rig.player.poll_event();
        rig.run(64);
        assert!(rig.snapshot().ended, "ended while paused");
        rig.player.seek(500).unwrap();
        rig.pump();
        rig.run(64);
        let s = rig.snapshot();
        assert!(s.ended);
        assert_eq!(s.generation, rig.player.generation());
    }

    #[test]
    fn seek_and_pause_in_one_buffer_keep_one_tail() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(100_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.player.play().unwrap();
        rig.run(1_024);
        rig.player.seek(9_000).unwrap();
        rig.player.pause().unwrap();
        rig.player.seek(7_000).unwrap();
        let out = rig.run(512);
        for (f, &v) in out[..FADE].iter().enumerate() {
            assert_eq!(
                v,
                ramp(1_024 + f) * (1.0 - f as f32 / FADE as f32),
                "frame {f}"
            );
        }
        let s = rig.snapshot();
        assert_eq!((s.anchor.track_frame, s.anchor.speed), (7_000, 0.0));
    }

    #[test]
    fn seek_past_the_end_while_paused_reads_as_ended() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(1_000)).unwrap();
        rig.pump();
        rig.loaded();
        rig.player.seek(1_000).unwrap();
        rig.pump();
        rig.run(64);
        let s = rig.snapshot();
        assert_eq!((s.anchor.track_frame, s.ended), (1_000, true));
    }

    #[test]
    fn buffer_with_a_partial_frame_is_safe() {
        let mut rig = Rig::new(256, 8);
        rig.player.load(ramp_track(20_000)).unwrap();
        rig.pump();
        rig.player.play().unwrap();
        let mut out = vec![9.0; 2 * 300 + 1];
        rig.source.process(
            &mut out,
            &CallbackInfo {
                frame: 0,
                timestamp_ns: 0,
                latency_ns: 0,
                sample_rate: RATE,
                channels: 2,
            },
        );
        assert_eq!(out[2 * 299], ramp(299));
        assert_eq!(rig.source.track_frame, 300);
    }

    /// Seeks a 440 Hz sine of `src_rate` Hz and returns, per target, the lag (output
    /// frames) at which the output best follows the sine on the 48 kHz grid.
    fn seek_lags(src_rate: u32, targets: &[u64]) -> Vec<f64> {
        let sine = |rate: u32, frame: f64| {
            ((frame / f64::from(rate) * 440.0) * std::f64::consts::TAU).sin() as f32
        };
        let data: Vec<f32> = (0..2 * src_rate as usize)
            .map(|n| sine(src_rate, n as f64))
            .collect();
        targets
            .iter()
            .map(|&target| {
                let mut rig = Rig::new(1_024, 8);
                rig.player.load(wav(src_rate, &data)).unwrap();
                rig.pump();
                rig.loaded();
                rig.player.seek(target).unwrap();
                rig.player.play().unwrap();
                rig.run(0);
                rig.pump();
                let mut out = Vec::new();
                for _ in 0..4 {
                    out.extend(rig.run(1_024));
                    rig.pump();
                }
                let error_at = |lag: f64| {
                    out.iter()
                        .enumerate()
                        .skip(FADE + 200)
                        .take(3_000)
                        .map(|(k, &v)| (v - sine(RATE, (target + k as u64) as f64 - lag)).abs())
                        .fold(0.0f32, f32::max)
                };
                let (error, lag) = (-40..=40)
                    .map(|i| {
                        let lag = f64::from(i) / 20.0;
                        (error_at(lag), lag)
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .unwrap();
                assert!(error < 0.01, "{src_rate} Hz, {target}: max error {error}");
                lag
            })
            .collect()
    }

    #[test]
    fn upsampled_seek_lands_within_half_a_frame() {
        // 22 050 → 48 000 (FFT resampler): a source frame spans 2.2 output frames.
        let targets = [0, 24_000, 24_001, 24_002, 24_003, 30_000];
        for (target, lag) in targets.iter().zip(seek_lags(22_050, &targets)) {
            assert!(lag.abs() <= 0.55, "{target}: lag {lag}");
        }
    }

    #[test]
    fn sinc_resampled_seek_lands_within_half_a_frame() {
        let targets = [0, 24_000, 24_001, 30_000];
        let lags = seek_lags(44_056, &targets);
        for (target, lag) in targets.iter().zip(lags) {
            assert!(lag.abs() <= 0.55, "{target}: lag {lag}");
        }
    }
}
