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
//!
//! The track rate (ADR-0036) is applied by the decoder with a pitch-preserving
//! time-stretch. A rate change re-renders the track from the start of a block a little
//! ahead of the callback, in a new epoch; the callback drops the replaced blocks and
//! carries on into the new ones without a gap.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Indexing, Resampler};
use signalsmith_stretch::Stretch;
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

/// Slowest track rate. Lazer's `TrackBass` rejects tempos below 0.05.
pub const MIN_MUSIC_RATE: f64 = 0.05;
/// Fastest track rate. Lazer sets no limit; its rate mods reach 2.
pub const MAX_MUSIC_RATE: f64 = 4.0;

/// A rate change replaces blocks from at least this long after the block the callback
/// plays (and at least two blocks on): time for the decoder to seek and render the
/// first new block, milliseconds.
const SPLICE_LEAD_MS: u64 = 40;

/// Blocks beyond the configured pool that the decoder fills only while a rate change
/// is in flight, when the queue is still full of blocks about to be replaced.
const SPLICE_RESERVE: usize = 4;

/// Output frames the time-stretch renders per call.
const STRETCH_CHUNK: usize = 1_024;

/// Track frames within which two positions count as the same (rounding of `f64`).
const FRAME_EPSILON: f64 = 1e-3;

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
    /// Commands that can wait for the audio callback and for the decoder each, 2..=255.
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
    /// Rate changes whose first block came after the callback had passed its start:
    /// the callback skipped ahead in the new blocks (a short fade-in, no time jump).
    pub late_splices: AtomicU64,
}

/// A stretch of the track timeline: from stream frame `device_frame` on, the track
/// advances `speed` track frames per stream frame from `track_frame`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MusicAnchor {
    /// Stream frame where the stretch starts, on the axis of [`CallbackInfo::frame`] and
    /// [`PositionSnapshot::frame`](crate::PositionSnapshot::frame).
    pub device_frame: u64,
    /// Track frame (at the stream rate) at `device_frame`; fractional when the track
    /// rate is not 1.
    pub track_frame: f64,
    /// The track rate while playing; 0 while paused, waiting for decoded audio or
    /// ended.
    pub speed: f64,
}

impl MusicAnchor {
    fn at(&self, device_frame: f64) -> f64 {
        self.track_frame + (device_frame - self.device_frame as f64) * self.speed
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
            self.previous.track_frame
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
            s.anchor.track_frame.to_bits(),
            s.anchor.speed.to_bits(),
            s.previous.device_frame,
            s.previous.track_frame.to_bits(),
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
                track_frame: f64::from_bits(w[1]),
                speed: f64::from_bits(w[2]),
            },
            previous: MusicAnchor {
                device_frame: w[3],
                track_frame: f64::from_bits(w[4]),
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
    /// Restart of the decoder (load, seek or rate change) that rendered the block.
    epoch: u32,
    /// Position in the generation: rate changes replace blocks from a given one on.
    seq: u32,
    /// Track frame of the first frame.
    start: f64,
    /// Track frames per frame.
    rate: f64,
    /// No blocks of this generation follow.
    end: bool,
}

/// Where the callback and the decoder are, for rate changes. Single atomic words
/// ([`Mark`]): the callback never waits for the decoder.
#[derive(Debug, Default)]
struct Splices {
    /// Written by the callback: the block it plays.
    cursor: AtomicU64,
    /// Written by the decoder: blocks of the generation from `seq` on, of epochs older
    /// than `epoch`, are replaced.
    splice: AtomicU64,
}

/// Block numbers wrap at 24 bits: one generation (load or seek) must not outlast 2^24
/// blocks, 97 hours of 1024-frame blocks at 48 kHz.
const SEQ_MASK: u32 = (1 << 24) - 1;

/// A block position in one word: epoch (32 bits), low 8 bits of the generation, `seq`
/// (24 bits). Epochs grow across generations; the generation bits keep a splice of a
/// newer generation (its flush still queued) off the blocks being played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mark {
    epoch: u32,
    generation: u8,
    seq: u32,
}

impl Mark {
    fn new(epoch: u32, generation: u32, seq: u32) -> Self {
        Self {
            epoch,
            generation: generation as u8,
            seq: seq & SEQ_MASK,
        }
    }

    fn store(self, word: &AtomicU64) {
        let w =
            u64::from(self.epoch) << 32 | u64::from(self.generation) << 24 | u64::from(self.seq);
        word.store(w, Ordering::Release);
    }

    fn load(word: &AtomicU64) -> Self {
        let w = word.load(Ordering::Acquire);
        Self {
            epoch: (w >> 32) as u32,
            generation: (w >> 24) as u8,
            seq: w as u32 & SEQ_MASK,
        }
    }
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
    Rate(f64),
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
    // At most 255 queued commands: a rate change tells generations apart by their
    // low 8 bits, and the decoder can run ahead of the callback by that many seeks.
    if config.block_frames == 0 || config.blocks == 0 || !(2..=255).contains(&config.queue_capacity)
    {
        return Err(AudioError::InvalidConfig(
            "music needs blocks and room for 2..=255 queued commands",
        ));
    }
    let channels = usize::from(config.channels);
    let block_values = config
        .block_frames
        .checked_mul(channels)
        .ok_or(AudioError::InvalidConfig("music block too large"))?;
    // Every block is in exactly one place (a ring, the callback, the decoder), so rings
    // as large as the pool never fill.
    let pool = config.blocks + SPLICE_RESERVE;
    let (filled_tx, filled_rx) = rtrb::RingBuffer::new(pool);
    let (mut free_tx, free_rx) = rtrb::RingBuffer::new(pool);
    for _ in 0..pool {
        let block = Block {
            data: vec![0.0; block_values].into_boxed_slice(),
            frames: 0,
            generation: 0,
            epoch: 0,
            seq: 0,
            start: 0.0,
            rate: 1.0,
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
    let splices = Arc::new(Splices::default());
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
        epoch: 0,
        generation_epoch: 0,
        seq: 0,
        starts: vec![0.0; pool].into_boxed_slice(),
        next_start: 0.0,
        rate: 1.0,
        target_rate: 1.0,
        splicing: false,
        // ≥ 2 blocks and ≥ SPLICE_LEAD_MS.
        splice_lead: (u64::from(config.sample_rate) * SPLICE_LEAD_MS / 1_000)
            .div_ceil(config.block_frames as u64)
            .clamp(2, u64::from(SEQ_MASK / 2)) as u32,
        splices: Arc::clone(&splices),
        stream: None,
        stretcher: None,
        finished: false,
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
        epoch: None,
        track_frame: 0.0,
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
        splices,
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
        rate: 1.0,
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
    /// Epoch of the blocks being played; `None` until the generation's first block.
    epoch: Option<u32>,
    /// Track frame of the next frame to play.
    track_frame: f64,
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
    splices: Arc<Splices>,
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
                    let rate = self.current.as_ref().map_or(1.0, |b| b.rate);
                    self.mark(info.frame + i as u64, rate);
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
                    self.track_frame = block.start + self.cursor as f64 * rate;
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
        let continues = a.speed == speed
            && device_frame >= a.device_frame
            && (a.at(device_frame as f64) - self.track_frame).abs() <= FRAME_EPSILON;
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
                    self.epoch = None;
                    self.track_frame = frame as f64;
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
                Ok(block) if self.replaced(block) => {
                    if let Ok(block) = self.filled.pop() {
                        self.recycle(block);
                    }
                }
                Ok(_) => {
                    let Ok(block) = self.filled.pop() else {
                        unreachable!("peeked above");
                    };
                    if !self.enter(&block) {
                        // Wholly behind the track: a late rate change.
                        self.recycle(block);
                        continue;
                    }
                    self.current = Some(block);
                    self.primed = true;
                }
            }
        }
    }

    /// `block` (of the current generation) was rendered before a rate change that
    /// replaces it, or belongs to an epoch the callback has left.
    fn replaced(&self, block: &Block) -> bool {
        if self.epoch.is_some_and(|e| is_older(block.epoch, e)) {
            return true;
        }
        let s = Mark::load(&self.splices.splice);
        s.generation == self.generation as u8
            && is_older(block.epoch, s.epoch)
            && block.seq >= s.seq
    }

    /// Starts playing `block` where the track is: false if the track has already
    /// passed all of it.
    fn enter(&mut self, block: &Block) -> bool {
        self.cursor = 0;
        // A rate change is rendered from a whole frame at or after the replaced block's
        // start, so the track continues forward by under a frame. A late one starts
        // behind the track: skip ahead to it.
        let behind = self.track_frame - block.start;
        if self.epoch.is_some() && behind > FRAME_EPSILON && block.frames > 0 {
            let skip = (behind / block.rate).ceil();
            if self.epoch != Some(block.epoch) {
                self.stats.late_splices.fetch_add(1, Ordering::Relaxed);
            }
            self.epoch = Some(block.epoch);
            self.env = 0.0;
            if skip < block.frames as f64 {
                self.cursor = skip as usize;
            } else if block.end {
                // Past the end: stay on the end block so the track reads as ended.
                self.cursor = block.frames;
                self.publish_cursor(block);
                return true;
            } else {
                return false;
            }
        } else {
            debug_assert!(
                block.frames == 0
                    || self.epoch != Some(block.epoch)
                    || behind.abs() <= FRAME_EPSILON,
                "decoder and callback disagree on the track frame"
            );
        }
        self.epoch = Some(block.epoch);
        self.track_frame = block.start + self.cursor as f64 * block.rate;
        self.publish_cursor(block);
        true
    }

    fn publish_cursor(&self, block: &Block) {
        Mark::new(block.epoch, self.generation, block.seq).store(&self.splices.cursor);
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
    rate: f64,
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

    /// Sets the track rate: track frames per output frame, with the pitch kept
    /// (lazer's `Tempo`). It stays across loads and seeks.
    ///
    /// The decoder re-renders the track from a block at least 40 ms (and two blocks)
    /// ahead of the one playing, so the change is heard some 40–60 ms later, without a
    /// gap; [`MusicSnapshot`] anchors carry the rate from
    /// the frame it is heard. While an earlier change is still on its way, the latest
    /// rate waits for it.
    pub fn set_rate(&mut self, rate: f64) -> Result<(), PlayError> {
        if !(MIN_MUSIC_RATE..=MAX_MUSIC_RATE).contains(&rate) {
            return Err(PlayError::InvalidRate);
        }
        self.send(&[], Some(DecoderCommand::Rate(rate)))?;
        self.rate = rate;
        Ok(())
    }

    /// The last rate set with [`set_rate`](Self::set_rate).
    pub fn rate(&self) -> f64 {
        self.rate
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
    /// Restarts so far (wrapping): loads, seeks, unloads and rate changes.
    epoch: u32,
    /// Epoch the current generation started with.
    generation_epoch: u32,
    /// `seq` of the next block.
    seq: u32,
    /// Track frame of block `seq` at `seq % len`, valid for the blocks in flight.
    starts: Box<[f64]>,
    /// Track frame of the next block.
    next_start: f64,
    /// Rate being rendered.
    rate: f64,
    /// Rate asked for; differs from `rate` until the change can be spliced in.
    target_rate: f64,
    /// A rate change was spliced in and the callback has not reached it yet: the
    /// reserve blocks may be filled, and the next change waits.
    splicing: bool,
    /// Blocks between the one the callback plays and the first one a rate change
    /// replaces.
    splice_lead: u32,
    splices: Arc<Splices>,
    stream: Option<Stream>,
    /// Time-stretch stage, created for the first rate other than 1.
    stretcher: Option<Stretcher>,
    /// The end block of the current epoch was queued.
    finished: bool,
    /// Queue an empty end block (the track failed to open).
    pending_end: bool,
    /// Block rendered but not queued yet.
    block: Option<Block>,
}

impl Decoder {
    fn run(mut self) {
        while !self.quit.load(Ordering::Relaxed) && !self.filled.is_abandoned() {
            while let Ok(command) = self.commands.pop() {
                self.handle(command);
            }
            self.splice();
            if !self.fill() {
                let busy =
                    (self.stream.is_some() && !self.finished) || self.rate != self.target_rate;
                std::thread::park_timeout(if busy { DECODER_POLL } else { IDLE_POLL });
            }
        }
    }

    fn handle(&mut self, command: DecoderCommand) {
        match command {
            DecoderCommand::Load { generation, source } => {
                self.begin(generation);
                self.stream = None;
                self.pending_end = false;
                match Stream::open(source, self.sample_rate, self.channels) {
                    Ok(stream) => {
                        let length = stream.length;
                        self.stream = Some(stream);
                        // A fresh stream already stands at frame 0.
                        self.restart(0, false);
                        self.event(MusicEvent::Loaded { generation, length });
                    }
                    Err(error) => {
                        self.restart(0, false);
                        self.pending_end = true;
                        self.event(MusicEvent::Failed { generation, error });
                    }
                }
            }
            DecoderCommand::Seek { generation, frame } => {
                self.begin(generation);
                self.restart(frame, true);
                // Failed or unloaded: the new generation ends at once.
                if self.stream.is_none() {
                    self.pending_end = true;
                }
            }
            DecoderCommand::Unload { generation } => {
                self.begin(generation);
                self.stream = None;
                self.pending_end = false;
                self.restart(0, false);
            }
            DecoderCommand::Rate(rate) => self.target_rate = rate,
        }
    }

    /// Starts generation `generation` from its first block, at the asked rate.
    fn begin(&mut self, generation: u32) {
        self.generation = generation;
        self.epoch = self.epoch.wrapping_add(1);
        self.generation_epoch = self.epoch;
        self.seq = 0;
        self.splicing = false;
        self.rate = self.target_rate;
    }

    /// Renders from track frame `frame` at `rate` on; `seek` moves the stream there.
    fn restart(&mut self, frame: u64, seek: bool) {
        self.finished = false;
        self.next_start = frame as f64;
        let Some(stream) = &mut self.stream else {
            return;
        };
        if seek {
            stream.seek(frame);
        }
        if self.rate != 1.0 {
            let (channels, rate) = (self.channels, self.sample_rate);
            self.stretcher
                .get_or_insert_with(|| Stretcher::new(channels, rate))
                .restart(frame, self.rate);
        }
    }

    /// Splices in a rate change (ADR-0036): re-renders from a block a little ahead of
    /// the callback, in a new epoch. Waits while an earlier change is on its way.
    fn splice(&mut self) {
        if self.rate == self.target_rate {
            return;
        }
        if self.stream.is_none() {
            // Nothing rendered at a rate: the next load or seek uses it.
            self.rate = self.target_rate;
            return;
        }
        if self.block.is_some() {
            // A rendered block waits for room in the ring (never: it holds the pool).
            return;
        }
        let cursor = Mark::load(&self.splices.cursor);
        // Epochs grow across generations: an older one is a generation the callback
        // has not left yet.
        let entered = !is_older(cursor.epoch, self.generation_epoch);
        if self.splicing {
            if entered && cursor.epoch == self.epoch {
                self.splicing = false;
            } else {
                return;
            }
        }
        let seq = if !entered {
            // The callback has not reached this generation: replace all of it.
            0
        } else if cursor.epoch == self.epoch {
            cursor.seq + self.splice_lead
        } else {
            return;
        };
        if seq >= self.seq && self.finished {
            // Everything to the end is queued at the old rate and plays before it.
            self.rate = self.target_rate;
            return;
        }
        if self.free.is_empty() {
            // The first new block could not be rendered before announcing the change;
            // try again once the callback returns blocks.
            return;
        }
        let seq = seq.min(self.seq);
        let start = if seq == self.seq {
            self.next_start
        } else {
            self.starts[seq as usize % self.starts.len()]
        };
        self.epoch = self.epoch.wrapping_add(1);
        self.seq = seq;
        self.rate = self.target_rate;
        self.splicing = true;
        // From a whole frame at or after the replaced block: the track moves on by
        // under a frame where the callback joins the new blocks.
        self.restart(start.ceil() as u64, true);
        // Render the first new block before announcing the change: until then the
        // callback keeps playing the old blocks rather than running dry.
        self.render();
        Mark::new(self.epoch, self.generation, seq).store(&self.splices.splice);
        self.queue();
    }

    fn event(&mut self, event: MusicEvent) {
        if self.events.push(event).is_err() {
            self.stats.dropped_events.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Fills and queues one block; false when there is nothing to do or no free block.
    fn fill(&mut self) -> bool {
        (self.block.is_some() || self.render()) && self.queue()
    }

    /// Renders the next block into `block`; false when there is nothing to do or no
    /// free block.
    fn render(&mut self) -> bool {
        let active = (self.stream.is_some() && !self.finished) || self.pending_end;
        if !active || self.block.is_some() {
            return false;
        }
        if !self.splicing && self.free.slots() <= SPLICE_RESERVE {
            // The reserve is for rate changes.
            return false;
        }
        let Ok(mut block) = self.free.pop() else {
            return false;
        };
        block.generation = self.generation;
        block.epoch = self.epoch;
        block.seq = self.seq;
        block.frames = 0;
        block.start = self.next_start;
        block.rate = self.rate;
        block.end = false;
        match &mut self.stream {
            Some(stream) if !self.pending_end => {
                let result = match &mut self.stretcher {
                    Some(stretcher) if self.rate != 1.0 => {
                        stretcher.fill(stream, &mut block, &self.stats)
                    }
                    _ => fill_plain(stream, &mut block, usize::from(self.channels), &self.stats),
                };
                if let Err(error) = result {
                    block.end = true;
                    stream.finished = true;
                    let generation = self.generation;
                    self.event(MusicEvent::Failed { generation, error });
                }
            }
            _ => {
                block.end = true;
                self.pending_end = false;
            }
        }
        self.block = Some(block);
        true
    }

    /// Queues the rendered block; false if there is none or the ring is full.
    fn queue(&mut self) -> bool {
        let Some(block) = self.block.take() else {
            return false;
        };
        let (start, frames, rate, end) = (block.start, block.frames, block.rate, block.end);
        if let Err(rtrb::PushError::Full(block)) = self.filled.push(block) {
            // Unreachable: the ring holds the whole pool. Keep it for the next round.
            self.block = Some(block);
            return false;
        }
        self.starts[self.seq as usize % self.starts.len()] = start;
        self.seq = (self.seq + 1) & SEQ_MASK;
        self.next_start = start + frames as f64 * rate;
        self.finished = end;
        self.stats.decoded_blocks.fetch_add(1, Ordering::Relaxed);
        true
    }
}

/// Fills `block` with the stream's audio as it is (rate 1).
fn fill_plain(
    stream: &mut Stream,
    block: &mut Block,
    ch: usize,
    stats: &MusicStats,
) -> Result<(), MusicError> {
    block.start = stream.next_frame as f64;
    let capacity = block.data.len() / ch;
    while block.frames < capacity {
        match stream.read(&mut block.data[block.frames * ch..], stats)? {
            0 => {
                block.end = true;
                break;
            }
            n => block.frames += n,
        }
    }
    Ok(())
}

/// Pitch-preserving time-stretch of the stream (rates other than 1).
///
/// After a restart at track frame `origin`, the stretch is primed with its input
/// latency of audio from `origin` and its output latency is dropped: kept output frame
/// `k` is then centred on track frame `origin + k · rate`. Input is fed cumulatively
/// (`round(rendered · rate)` frames for `rendered` output frames) so no rounding
/// builds up; past the end it is silence, up to `ceil(remaining / rate)` output frames.
struct Stretcher {
    stretch: Stretch,
    channels: usize,
    rate: f64,
    /// Track frame of the restart.
    origin: f64,
    /// The input latency has been fed.
    primed: bool,
    /// Output frames still to drop: the output latency.
    drop: usize,
    /// Output frames rendered since the restart, dropped ones included.
    rendered: u64,
    /// Output frames kept since the restart.
    kept: u64,
    /// Track frames read from `origin` on.
    read: u64,
    /// The stream ended after `read` frames.
    ended: bool,
    input: Vec<f32>,
    output: Vec<f32>,
}

impl Stretcher {
    fn new(channels: u16, sample_rate: u32) -> Self {
        Self {
            stretch: Stretch::preset_default(u32::from(channels), sample_rate),
            channels: usize::from(channels),
            rate: 1.0,
            origin: 0.0,
            primed: false,
            drop: 0,
            rendered: 0,
            kept: 0,
            read: 0,
            ended: false,
            input: Vec::new(),
            output: Vec::new(),
        }
    }

    /// Starts over at track frame `origin` (where the stream now stands) at `rate`.
    fn restart(&mut self, origin: u64, rate: f64) {
        self.stretch.reset();
        self.rate = rate;
        self.origin = origin as f64;
        self.primed = false;
        self.drop = self.stretch.output_latency();
        self.rendered = 0;
        self.kept = 0;
        self.read = 0;
        self.ended = false;
    }

    /// Output frames from the restart to the end of the track, once it is known.
    fn total(&self) -> Option<u64> {
        self.ended
            .then(|| (self.read as f64 / self.rate).ceil() as u64)
    }

    /// Reads `frames` input frames into `input`, silence after the end.
    fn take(
        &mut self,
        stream: &mut Stream,
        frames: usize,
        stats: &MusicStats,
    ) -> Result<(), MusicError> {
        let ch = self.channels;
        self.input.clear();
        self.input.resize(frames * ch, 0.0);
        let mut got = 0;
        while !self.ended && got < frames {
            match stream.read(&mut self.input[got * ch..], stats)? {
                0 => self.ended = true,
                n => got += n,
            }
        }
        self.read += got as u64;
        Ok(())
    }

    fn fill(
        &mut self,
        stream: &mut Stream,
        block: &mut Block,
        stats: &MusicStats,
    ) -> Result<(), MusicError> {
        let ch = self.channels;
        block.start = self.origin + self.kept as f64 * self.rate;
        if !self.primed {
            self.take(stream, self.stretch.input_latency(), stats)?;
            self.stretch.seek(&self.input, self.rate);
            self.primed = true;
        }
        let capacity = block.data.len() / ch;
        while block.frames < capacity {
            let left = self.total().map(|t| t.saturating_sub(self.kept));
            if left == Some(0) {
                block.end = true;
                break;
            }
            let mut want = (capacity - block.frames + self.drop).min(STRETCH_CHUNK);
            if let Some(left) = left {
                want = want.min(self.drop.saturating_add(left as usize));
            }
            let fed = (self.rendered as f64 * self.rate).round() as u64;
            let rendered = self.rendered + want as u64;
            let feed = (rendered as f64 * self.rate).round() as u64 - fed;
            self.take(stream, feed as usize, stats)?;
            self.output.clear();
            self.output.resize(want * ch, 0.0);
            self.stretch.process(&self.input, &mut self.output);
            self.rendered = rendered;
            let dropped = self.drop.min(want);
            self.drop -= dropped;
            let mut keep = want - dropped;
            if let Some(total) = self.total() {
                // The end may have come up in this read.
                keep = keep.min(total.saturating_sub(self.kept) as usize);
            }
            let at = block.frames * ch;
            block.data[at..at + keep * ch]
                .copy_from_slice(&self.output[dropped * ch..(dropped + keep) * ch]);
            block.frames += keep;
            self.kept += keep as u64;
        }
        Ok(())
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
            if keep as u64 == room {
                // Nothing more may follow: without this, a length cap that bites
                // before `emitted` reaches the computed length would resample silence
                // forever.
                self.finished = true;
            }
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
                self.decoder.splice();
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
                // The time-stretch processes channels jointly: not bit-identical.
                assert!(
                    (frame[0] - frame[1]).abs() <= 1e-3,
                    "mono must reach both channels: {frame:?}"
                );
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
            (0.0, 0.0)
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
            (64, 0.0, 1.0)
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
        assert_eq!((s.anchor.track_frame, s.anchor.speed), (50_000.0, 0.0));
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
            (1_536, 50_000.0, 1.0)
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
            (512, 512.0, 0.0)
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
        assert_eq!(rig.snapshot().anchor.track_frame, 512.0);
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
            (700, 0.0, 1.0)
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
            (512, 0.0, 1.0)
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
            (512, 512.0, 0.0)
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
            (1_000, 1_000.0, 0.0)
        );
        assert!(s.ended);
        assert_eq!(rig.source.stats.underruns.load(Ordering::Relaxed), 0);

        rig.player.seek(5_000).unwrap();
        rig.pump();
        rig.run(256);
        let s = rig.snapshot();
        assert_eq!(
            (s.anchor.track_frame, s.anchor.speed, s.ended),
            (1_000.0, 0.0, true)
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
            (1_024, 1_024.0, 0.0)
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
        assert_eq!(s.anchor.track_frame, 3.0 * f64::from(RATE));
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
            MusicConfig {
                queue_capacity: 256,
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
        assert_eq!((s.anchor.track_frame, s.ended), (1_000.0, true));
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
        assert_eq!((s.anchor.track_frame, s.anchor.speed), (7_000.0, 0.0));
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
        assert_eq!((s.anchor.track_frame, s.ended), (1_000.0, true));
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
        assert_eq!(rig.source.track_frame, 300.0);
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

    /// Mono sine of `period` frames.
    fn sine(frames: usize, period: f32) -> Vec<f32> {
        (0..frames)
            .map(|f| (f as f32 / period * std::f32::consts::TAU).sin() * 0.5)
            .collect()
    }

    /// Silence with a decaying 2 kHz burst starting at `at`.
    fn burst_track(frames: usize, at: usize) -> TrackSource {
        let mut data = vec![0.0; frames];
        for (i, v) in data[at..at + 480].iter_mut().enumerate() {
            let t = i as f32 / RATE as f32;
            *v = (t * 2_000.0 * std::f32::consts::TAU).sin() * (1.0 - i as f32 / 480.0);
        }
        wav(RATE, &data)
    }

    /// Plays the loaded track from stream frame 0 in buffers of 256 frames until it
    /// ends; returns the left channel and the snapshot after each buffer.
    fn play_out(rig: &mut Rig) -> (Vec<f32>, Vec<MusicSnapshot>) {
        rig.player.play().unwrap();
        let (mut out, mut snaps) = (Vec::new(), Vec::new());
        while !rig.snapshot_or_none().is_some_and(|s| s.ended) {
            rig.pump();
            out.extend(rig.run(256));
            snaps.push(rig.snapshot());
            assert!(out.len() < 10 * RATE as usize, "track does not end");
        }
        (out, snaps)
    }

    impl Rig {
        fn snapshot_or_none(&self) -> Option<MusicSnapshot> {
            self.player.position().read()
        }
    }

    /// Track frame heard at the loudest output frame of a burst track.
    fn burst_position(rig: &mut Rig) -> f64 {
        let (out, _) = play_out(rig);
        let peak = (0..out.len())
            .max_by(|&a, &b| out[a].abs().total_cmp(&out[b].abs()))
            .unwrap();
        // The whole track plays in one stretch: the anchor before the end holds it.
        let s = rig.snapshot();
        s.previous.at(peak as f64)
    }

    #[test]
    fn stretched_track_keeps_pitch_and_ends_after_length_over_rate() {
        for rate in [0.75, 1.5, 2.0] {
            let mut rig = Rig::new(1_024, 8);
            rig.player.set_rate(rate).unwrap();
            rig.player.load(wav(RATE, &sine(96_000, 100.0))).unwrap();
            let (out, snaps) = play_out(&mut rig);
            let end = rig.snapshot();
            assert_eq!(end.anchor.track_frame, 96_000.0, "rate {rate}");
            let played = (96_000.0 / rate).ceil() as u64;
            assert_eq!(end.anchor.device_frame, played, "rate {rate}");
            assert!(
                snaps
                    .iter()
                    .all(|s| s.anchor.speed == rate || s.anchor.speed == 0.0)
            );
            // Pitch: rising zero crossings 100 frames apart, as in the source.
            let mid = &out[out.len() / 4..out.len() / 2];
            let rising: Vec<usize> = (1..mid.len())
                .filter(|&i| mid[i - 1] < 0.0 && mid[i] >= 0.0)
                .collect();
            let period = (rising[rising.len() - 1] - rising[0]) as f64 / (rising.len() - 1) as f64;
            assert!((period - 100.0).abs() < 0.5, "rate {rate}: period {period}");
            assert_eq!(rig.player.stats().underruns.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn stretched_audio_lines_up_with_the_track_position() {
        for rate in [0.75, 1.5] {
            let mut rig = Rig::new(1_024, 8);
            rig.player.set_rate(rate).unwrap();
            rig.player.load(burst_track(96_000, 60_000)).unwrap();
            let at = burst_position(&mut rig);
            // The burst decays from its start: its peak is heard within 2 ms of it.
            assert!(
                (60_000.0..60_096.0).contains(&at),
                "rate {rate}: heard at {at}"
            );
        }
    }

    #[test]
    fn seek_while_stretched_lines_up() {
        let mut rig = Rig::new(1_024, 8);
        rig.player.set_rate(1.5).unwrap();
        rig.player.load(burst_track(96_000, 60_000)).unwrap();
        rig.player.seek(41_001).unwrap();
        let (out, _) = play_out(&mut rig);
        let s = rig.snapshot();
        assert_eq!(s.previous.track_frame, 41_001.0);
        let peak = (0..out.len())
            .max_by(|&a, &b| out[a].abs().total_cmp(&out[b].abs()))
            .unwrap();
        let at = s.previous.at(peak as f64);
        assert!((60_000.0..60_096.0).contains(&at), "heard at {at}");
    }

    /// Plays `buffers` buffers of 256 frames, checking that the written track position
    /// moves on without gaps or steps back; returns the speeds seen.
    fn play_continuously(rig: &mut Rig, buffers: usize, last: &mut f64) -> Vec<f64> {
        let mut speeds = Vec::new();
        for _ in 0..buffers {
            rig.pump();
            let frame = rig.frame as f64;
            rig.run(256);
            let s = rig.snapshot();
            let at = s.track_frame_at(frame);
            assert!(
                at >= *last - FRAME_EPSILON && at <= *last + 1.0,
                "track jumps from {last} to {at}"
            );
            *last = s.track_frame_at(frame + 256.0);
            speeds.push(s.anchor.speed);
        }
        speeds
    }

    #[test]
    fn rate_changes_while_playing_join_without_a_gap() {
        let mut rig = Rig::new(1_024, 8);
        rig.player.load(ramp_track(400_000)).unwrap();
        rig.pump();
        rig.player.play().unwrap();
        let mut last = 0.0;
        play_continuously(&mut rig, 20, &mut last);
        rig.player.set_rate(1.5).unwrap();
        let speeds = play_continuously(&mut rig, 30, &mut last);
        // Heard from the block after next: within three blocks of 4 buffers.
        let switched = speeds.iter().position(|&s| s == 1.5).unwrap();
        assert!(switched <= 12, "rate 1.5 after {switched} buffers");
        assert!(speeds[switched..].iter().all(|&s| s == 1.5));
        rig.player.set_rate(1.0).unwrap();
        let speeds = play_continuously(&mut rig, 30, &mut last);
        assert_eq!(speeds.last(), Some(&1.0));
        // Back at rate 1 the track plays exactly again.
        let frame = rig.frame as f64;
        let out = rig.run(256);
        let start = rig.snapshot().track_frame_at(frame);
        assert_eq!(start.fract(), 0.0);
        for (i, v) in out.iter().enumerate() {
            assert_eq!(*v, ramp(start as usize + i));
        }
        let stats = rig.player.stats();
        assert_eq!(stats.underruns.load(Ordering::Relaxed), 0);
        assert_eq!(stats.late_splices.load(Ordering::Relaxed), 0);
        assert_eq!(stats.leaked_blocks.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn rate_set_before_playing_replaces_the_whole_generation() {
        let mut rig = Rig::new(1_024, 8);
        rig.player.load(ramp_track(48_000)).unwrap();
        rig.pump();
        rig.player.set_rate(2.0).unwrap();
        rig.player.play().unwrap();
        let mut last = 0.0;
        let speeds = play_continuously(&mut rig, 4, &mut last);
        assert!(speeds.iter().all(|&s| s == 2.0), "{speeds:?}");
    }

    #[test]
    fn rate_changes_wait_for_the_previous_one() {
        let mut rig = Rig::new(1_024, 8);
        rig.player.load(ramp_track(400_000)).unwrap();
        rig.pump();
        rig.player.play().unwrap();
        let mut last = 0.0;
        play_continuously(&mut rig, 4, &mut last);
        rig.player.set_rate(1.5).unwrap();
        rig.pump();
        rig.player.set_rate(0.75).unwrap();
        rig.pump();
        assert_eq!(rig.decoder.rate, 1.5, "second change waits");
        let speeds = play_continuously(&mut rig, 40, &mut last);
        assert!(speeds.contains(&1.5));
        assert_eq!(speeds.last(), Some(&0.75));
        assert_eq!(rig.player.stats().late_splices.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn late_rate_change_skips_ahead_without_stepping_back() {
        let mut rig = Rig::new(1_024, 8);
        rig.player.load(ramp_track(400_000)).unwrap();
        rig.pump();
        rig.player.play().unwrap();
        let mut last = 0.0;
        play_continuously(&mut rig, 20, &mut last);
        // The decoder reads a cursor three blocks old: it splices at a block the
        // callback has already started.
        let mut stale = Mark::load(&rig.source.splices.cursor);
        stale.seq -= 3;
        stale.store(&rig.source.splices.cursor);
        rig.player.set_rate(1.5).unwrap();
        rig.pump();
        let speeds = play_continuously(&mut rig, 30, &mut last);
        assert_eq!(speeds.last(), Some(&1.5));
        assert_eq!(rig.player.stats().late_splices.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn rejects_rates_out_of_range() {
        let mut rig = Rig::new(256, 4);
        for bad in [0.0, 0.049, 4.01, f64::NAN, f64::INFINITY] {
            assert_eq!(rig.player.set_rate(bad), Err(PlayError::InvalidRate));
        }
        assert_eq!(rig.player.rate(), 1.0);
        rig.player.set_rate(MAX_MUSIC_RATE).unwrap();
        assert_eq!(rig.player.rate(), MAX_MUSIC_RATE);
    }

    /// Seeks and restarts near the end of a resampled track used to leave the stream
    /// resampling silence forever once the length cap bit (a hang, not a failure).
    #[test]
    fn seeks_and_rate_changes_on_a_vorbis_track_keep_decoding() {
        let path = format!(
            "{}/tests/data/tone-vorbis-stereo.ogg",
            env!("CARGO_MANIFEST_DIR")
        );
        let mut rig = Rig::new(256, 4);
        rig.player.load(TrackSource::File(path.into())).unwrap();
        rig.pump();
        for round in 0..200u64 {
            match round % 6 {
                0 => rig.player.play().unwrap(),
                1 => rig.player.seek(round * 37 % 4_000).unwrap(),
                3 => rig.player.pause().unwrap(),
                4 => rig
                    .player
                    .set_rate(if round % 12 == 4 { 1.5 } else { 1.0 })
                    .unwrap(),
                _ => {}
            }
            rig.pump();
            rig.run(1_217);
        }
    }

    #[test]
    fn the_first_new_block_is_ready_when_a_rate_change_is_announced() {
        let mut rig = Rig::new(1_024, 8);
        rig.player.load(ramp_track(400_000)).unwrap();
        rig.pump();
        rig.player.play().unwrap();
        let mut last = 0.0;
        play_continuously(&mut rig, 10, &mut last);
        rig.player.set_rate(1.5).unwrap();
        // The decoder splices but fills nothing more: the callback plays on through
        // the lead blocks into the first new one without running dry.
        while let Ok(command) = rig.decoder.commands.pop() {
            rig.decoder.handle(command);
        }
        rig.decoder.splice();
        let mut speeds = Vec::new();
        for _ in 0..8 {
            let frame = rig.frame as f64;
            rig.run(256);
            let s = rig.snapshot();
            assert!(s.track_frame_at(frame) >= last - FRAME_EPSILON);
            last = s.track_frame_at(frame + 256.0);
            speeds.push(s.anchor.speed);
        }
        assert_eq!(speeds.last(), Some(&1.5), "{speeds:?}");
        assert_eq!(rig.player.stats().underruns.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn late_rate_change_at_the_very_end_still_ends() {
        // 48 full blocks and an empty end block: the callback stands at the end when
        // the replaced end block is dropped, and every new block lies behind it.
        let mut rig = Rig::new(1_024, 8);
        rig.player.load(ramp_track(48 * 1_024)).unwrap();
        rig.pump();
        rig.player.play().unwrap();
        while rig.source.track_frame < 47.0 * 1_024.0 {
            rig.pump();
            rig.run(256);
        }
        let mut stale = Mark::load(&rig.source.splices.cursor);
        stale.seq -= 3;
        stale.store(&rig.source.splices.cursor);
        rig.player.set_rate(1.5).unwrap();
        rig.pump();
        for _ in 0..40 {
            rig.pump();
            rig.run(256);
        }
        let s = rig.snapshot();
        assert!(s.ended);
        assert!(s.anchor.track_frame >= 48.0 * 1_024.0 - FRAME_EPSILON);
        assert_eq!(rig.player.stats().late_splices.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn rate_change_while_paused_is_heard_after_resuming() {
        let mut rig = Rig::new(1_024, 8);
        rig.player.load(ramp_track(400_000)).unwrap();
        rig.pump();
        rig.player.play().unwrap();
        let mut last = 0.0;
        play_continuously(&mut rig, 10, &mut last);
        rig.player.pause().unwrap();
        play_continuously(&mut rig, 4, &mut last);
        rig.player.set_rate(0.75).unwrap();
        play_continuously(&mut rig, 4, &mut last);
        rig.player.play().unwrap();
        let speeds = play_continuously(&mut rig, 20, &mut last);
        assert_eq!(speeds.last(), Some(&0.75));
        let stats = rig.player.stats();
        assert_eq!(stats.underruns.load(Ordering::Relaxed), 0);
        assert_eq!(stats.late_splices.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn seek_during_a_rate_change_plays_at_the_new_rate() {
        let mut rig = Rig::new(1_024, 8);
        rig.player.load(ramp_track(400_000)).unwrap();
        rig.pump();
        rig.player.play().unwrap();
        let mut last = 0.0;
        play_continuously(&mut rig, 10, &mut last);
        rig.player.set_rate(1.5).unwrap();
        rig.pump();
        rig.player.seek(200_000).unwrap();
        rig.pump();
        assert!(!rig.decoder.splicing);
        rig.run(256);
        let mut last = rig.snapshot().track_frame_at(rig.frame as f64);
        let speeds = play_continuously(&mut rig, 20, &mut last);
        assert!(speeds.iter().all(|&s| s == 1.5), "{speeds:?}");
        assert!(last > 200_000.0);
        assert_eq!(rig.player.stats().underruns.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn rate_change_waits_for_a_free_block_to_render_into() {
        let mut rig = Rig::new(1_024, 8);
        rig.player.load(ramp_track(400_000)).unwrap();
        rig.pump();
        rig.player.play().unwrap();
        let mut last = 0.0;
        play_continuously(&mut rig, 10, &mut last);
        let mut taken = Vec::new();
        while let Ok(block) = rig.decoder.free.pop() {
            taken.push(block);
        }
        rig.player.set_rate(1.5).unwrap();
        rig.pump();
        assert_eq!(rig.decoder.rate, 1.0, "no block to render the change into");
        assert_eq!(Mark::load(&rig.source.splices.splice).epoch, 0);
        for block in taken {
            assert!(rig.source.free.push(block).is_ok());
        }
        let speeds = play_continuously(&mut rig, 30, &mut last);
        assert_eq!(speeds.last(), Some(&1.5));
        assert_eq!(rig.player.stats().underruns.load(Ordering::Relaxed), 0);
    }
}
