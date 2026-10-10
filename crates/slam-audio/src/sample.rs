//! Pre-decoded samples (hitsounds): decoding with `symphonia`, channel conversion and
//! resampling with `rubato` to the device format. Runs off the hot paths (loading).

use std::io::Cursor;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, Fft, FixedAsync, FixedSync, Resampler, SincInterpolationParameters};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Frames per resampler chunk: the size the `rubato` documentation suggests for offline
/// conversion with the FFT resampler.
const RESAMPLE_CHUNK: usize = 1024;

/// Most values (frames × channels) a decoded sample may hold: 5 minutes of stereo at
/// 48 kHz, 115 MB. Bounds the memory a corrupt or hostile file can take while decoding;
/// hitsounds are a few seconds.
pub const MAX_SAMPLE_VALUES: usize = 48_000 * 2 * 300;

/// Most channels a sample may have (as many as an output stream).
pub const MAX_CHANNELS: u16 = 8;

/// Sample rates accepted, Hz. Decoders take the rate from the file unchecked.
pub const SAMPLE_RATES: std::ops::RangeInclusive<u32> = 1_000..=384_000;

/// Largest reduced rate ratio term (`rate / gcd(from, to)`) resampled with the FFT
/// resampler, whose plan grows with it (44.1 → 48 kHz is 147:160). Other ratios use the
/// sinc resampler, whose memory does not depend on the ratio.
const MAX_FFT_RATIO_TERM: u32 = 2_048;

/// Errors from decoding or converting a sample.
#[derive(Debug, thiserror::Error)]
pub enum SampleError {
    #[error("unsupported or corrupt audio: {0}")]
    Decode(String),
    #[error("audio has no playable track")]
    NoTrack,
    #[error("sample over {MAX_SAMPLE_VALUES} values or {MAX_CHANNELS} channels")]
    TooLong,
    #[error("invalid sample format: {0}")]
    InvalidFormat(&'static str),
    #[error("resampling failed: {0}")]
    Resample(String),
}

/// Immutable PCM: interleaved `f32` at a fixed rate and channel count.
///
/// Share it as `Arc<Sample>` with the mixer; keep the owning reference outside the
/// audio thread (see [`MixerHandle`](crate::MixerHandle)).
#[derive(Clone, PartialEq)]
pub struct Sample {
    data: Box<[f32]>,
    sample_rate: u32,
    channels: u16,
}

impl Sample {
    /// Wraps interleaved PCM. `data.len()` must be a whole number of frames.
    /// Non-finite values become 0, and so do subnormals: they are slow to mix on x86
    /// and inaudible.
    pub fn from_interleaved(
        mut data: Vec<f32>,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, SampleError> {
        if sample_rate == 0 || channels == 0 {
            return Err(SampleError::InvalidFormat("zero rate or channels"));
        }
        if !SAMPLE_RATES.contains(&sample_rate) {
            return Err(SampleError::InvalidFormat(
                "sample rate out of 1000..=384000",
            ));
        }
        if channels > MAX_CHANNELS {
            return Err(SampleError::TooLong);
        }
        if !data.len().is_multiple_of(usize::from(channels)) {
            return Err(SampleError::InvalidFormat("partial frame"));
        }
        for v in &mut data {
            if !v.is_normal() {
                *v = 0.0;
            }
        }
        Ok(Self {
            data: data.into_boxed_slice(),
            sample_rate,
            channels,
        })
    }

    /// Decodes a whole file held in memory (WAV, OGG Vorbis, MP3) at its own rate and
    /// channel count. `extension` (without the dot) helps the format probe; the content
    /// decides. Encoder delay and padding are trimmed (gapless). Corrupt packets inside
    /// an otherwise valid stream are skipped.
    pub fn decode(bytes: &[u8], extension: Option<&str>) -> Result<Self, SampleError> {
        let decode_err = |e: SymphoniaError| SampleError::Decode(e.to_string());
        let source = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = extension {
            hint.with_extension(ext);
        }
        let mut format = symphonia::default::get_probe()
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
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or(SampleError::NoTrack)?;
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(decode_err)?;

        let mut data = Vec::new();
        let mut packet_samples = Vec::new();
        let mut spec: Option<(u32, u16)> = None;
        let mut last_error = None;
        loop {
            let packet = match format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) | Err(SymphoniaError::ResetRequired) => break,
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(e) => return Err(decode_err(e)),
            };
            if packet.track_id != track_id {
                continue;
            }
            let buffer = match decoder.decode(&packet) {
                Ok(buffer) => buffer,
                Err(e @ (SymphoniaError::DecodeError(_) | SymphoniaError::IoError(_))) => {
                    last_error = Some(e);
                    continue;
                }
                Err(e) => return Err(decode_err(e)),
            };
            let rate = buffer.spec().rate();
            let channels = u16::try_from(buffer.spec().channels().count())
                .ok()
                .filter(|c| (1..=MAX_CHANNELS).contains(c))
                .ok_or(SampleError::TooLong)?;
            match spec {
                None => spec = Some((rate, channels)),
                Some(s) if s != (rate, channels) => {
                    return Err(SampleError::InvalidFormat("format changes mid-stream"));
                }
                Some(_) => {}
            }
            // Checked before copying: a hostile packet cannot grow `data` past the limit.
            if data.len() + buffer.samples_interleaved() > MAX_SAMPLE_VALUES {
                return Err(SampleError::TooLong);
            }
            buffer.copy_to_vec_interleaved::<f32>(&mut packet_samples);
            data.extend_from_slice(&packet_samples);
        }
        // A file whose packets all failed or that holds no frames has no format either.
        let (rate, channels) = match (spec, last_error) {
            (Some(spec), _) => spec,
            (None, Some(e)) => return Err(decode_err(e)),
            (None, None) => return Err(SampleError::NoTrack),
        };
        Self::from_interleaved(data, rate, channels)
    }

    /// Decodes and converts to the output format in one step: what loading a hitsound
    /// needs.
    pub fn load(
        bytes: &[u8],
        extension: Option<&str>,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, SampleError> {
        Self::decode(bytes, extension)?.convert(sample_rate, channels)
    }

    /// Returns this sample at `sample_rate` and `channels`.
    ///
    /// Channels: mono is copied to every output channel, any layout down-mixes to mono
    /// by averaging, otherwise channels map by index (extra outputs silent, extra inputs
    /// dropped). Rate: band-limited resampling; the length becomes
    /// `ceil(frames · new_rate / old_rate)`, and the resampler delay is removed (the
    /// result is aligned to within one frame).
    pub fn convert(&self, sample_rate: u32, channels: u16) -> Result<Self, SampleError> {
        if !SAMPLE_RATES.contains(&sample_rate) || channels == 0 {
            return Err(SampleError::InvalidFormat(
                "rate out of range or zero channels",
            ));
        }
        if channels > MAX_CHANNELS {
            return Err(SampleError::TooLong);
        }
        // Checked before converting: upsampling a long file must not exhaust memory.
        let out_values = (self.frames() as u64 * u64::from(sample_rate))
            .div_ceil(u64::from(self.sample_rate))
            * u64::from(channels);
        if out_values > MAX_SAMPLE_VALUES as u64 {
            return Err(SampleError::TooLong);
        }
        // Resample at the smaller channel count: cheaper, and no intermediate buffer
        // larger than the source or the result.
        let resampled = |data: Vec<f32>, channels| {
            if sample_rate == self.sample_rate || data.is_empty() {
                Ok(data)
            } else {
                resample(&data, channels, self.sample_rate, sample_rate)
            }
        };
        let data = if channels < self.channels {
            resampled(remix(&self.data, self.channels, channels), channels)?
        } else {
            remix(
                &resampled(self.data.to_vec(), self.channels)?,
                self.channels,
                channels,
            )
        };
        Self::from_interleaved(data, sample_rate, channels)
    }

    /// Interleaved samples.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Frames per second.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Interleaved channels per frame.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Number of frames.
    pub fn frames(&self) -> usize {
        self.data.len() / usize::from(self.channels)
    }
}

impl std::fmt::Debug for Sample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sample")
            .field("frames", &self.frames())
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .finish()
    }
}

fn remix(data: &[f32], from: u16, to: u16) -> Vec<f32> {
    if from == to {
        return data.to_vec();
    }
    let (from, to) = (usize::from(from), usize::from(to));
    let frames = data.chunks_exact(from);
    let mut out = Vec::with_capacity(frames.len() * to);
    for frame in frames {
        if from == 1 {
            out.extend(std::iter::repeat_n(frame[0], to));
        } else if to == 1 {
            out.push(frame.iter().sum::<f32>() / from as f32);
        } else {
            out.extend((0..to).map(|c| frame.get(c).copied().unwrap_or(0.0)));
        }
    }
    out
}

fn resample(data: &[f32], channels: u16, from: u32, to: u32) -> Result<Vec<f32>, SampleError> {
    let channels = usize::from(channels);
    let frames = data.len() / channels;
    let err = |e: &dyn std::fmt::Display| SampleError::Resample(e.to_string());
    let input = InterleavedSlice::new(data, channels, frames).map_err(|e| err(&e))?;
    let gcd = {
        let (mut a, mut b) = (from, to);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    let mut resampler: Box<dyn Resampler<f32>> =
        if from / gcd <= MAX_FFT_RATIO_TERM && to / gcd <= MAX_FFT_RATIO_TERM {
            Box::new(
                Fft::<f32>::new(
                    from as usize,
                    to as usize,
                    RESAMPLE_CHUNK,
                    channels,
                    FixedSync::Both,
                )
                .map_err(|e| err(&e))?,
            )
        } else {
            Box::new(
                Async::<f32>::new_sinc(
                    f64::from(to) / f64::from(from),
                    1.0,
                    &SincInterpolationParameters::default(),
                    RESAMPLE_CHUNK,
                    channels,
                    FixedAsync::Input,
                )
                .map_err(|e| err(&e))?,
            )
        };
    // `process_all` trims the resampler delay and the padding of the last chunk.
    let output = resampler
        .process_all(&input, frames, None)
        .map_err(|e| err(&e))?;
    let mut data = output.take_data();
    // `rubato` rounds the length through floating point and can produce a frame more;
    // the exact length is `ceil(frames · to / from)`.
    let exact = (frames as u64 * u64::from(to)).div_ceil(u64::from(from)) as usize;
    data.truncate(exact * channels);
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_partial_frames_and_empty_format() {
        assert!(Sample::from_interleaved(vec![0.0; 3], 48_000, 2).is_err());
        assert!(Sample::from_interleaved(vec![], 0, 2).is_err());
        assert!(Sample::from_interleaved(vec![], 48_000, 0).is_err());
        assert_eq!(
            Sample::from_interleaved(vec![0.0; 4], 48_000, 2)
                .unwrap()
                .frames(),
            2
        );
    }

    #[test]
    fn cleans_values_and_limits_channels() {
        let s = Sample::from_interleaved(
            vec![f32::NAN, f32::INFINITY, 1e-40, -0.5, 0.0, f32::NEG_INFINITY],
            48_000,
            1,
        )
        .unwrap();
        assert_eq!(s.data(), [0.0, 0.0, 0.0, -0.5, 0.0, 0.0]);
        assert!(Sample::from_interleaved(vec![0.0; 9], 48_000, 9).is_err());
    }

    #[test]
    fn remixes_channels() {
        assert_eq!(remix(&[0.5, -0.5], 1, 2), [0.5, 0.5, -0.5, -0.5]);
        assert_eq!(remix(&[0.25, 0.75, -1.0, 0.0], 2, 1), [0.5, -0.5]);
        assert_eq!(remix(&[0.1, 0.2, 0.3], 3, 2), [0.1, 0.2]);
        assert_eq!(remix(&[0.1, 0.2], 2, 3), [0.1, 0.2, 0.0]);
        assert_eq!(remix(&[0.1, 0.2], 2, 2), [0.1, 0.2]);
    }

    /// Sine of `freq` Hz on a `rate` grid, delayed by `lag` frames.
    fn sine(freq: f64, rate: u32, frames: usize, lag: f64) -> Vec<f32> {
        (0..frames)
            .map(|n| {
                ((n as f64 - lag) * freq / f64::from(rate) * std::f64::consts::TAU).sin() as f32
            })
            .collect()
    }

    /// Resamples a 1 kHz sine from `from` to `to` Hz (mono to stereo) and checks the
    /// length and that the output follows the same sine on the new grid in both
    /// channels, offset by under one frame.
    fn check_resample(from: u32, to: u32) {
        let frames = from as usize;
        let src = Sample::from_interleaved(sine(1_000.0, from, frames, 0.0), from, 1).unwrap();
        let dst = src.convert(to, 2).unwrap();
        assert_eq!((dst.sample_rate(), dst.channels()), (to, 2));
        let out_frames = (frames as u64 * u64::from(to)).div_ceil(u64::from(from)) as usize;
        assert_eq!(dst.frames(), out_frames);
        let error_at = |lag: f64| {
            let want = sine(1_000.0, to, out_frames, lag);
            dst.data()
                .as_chunks::<2>()
                .0
                .iter()
                .zip(&want)
                .skip(2_000)
                .take(out_frames - 4_000)
                .map(|(frame, w)| (frame[0] - w).abs().max((frame[1] - w).abs()))
                .fold(0.0f32, f32::max)
        };
        let (error, lag) = (-20..=20)
            .map(|i| {
                let lag = f64::from(i) / 20.0;
                (error_at(lag), lag)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap();
        assert!(
            error < 0.01,
            "{from} -> {to}: max error {error} at lag {lag}"
        );
    }

    #[test]
    fn resampling_keeps_length_pitch_and_alignment() {
        // FFT resampler: common rates.
        check_resample(44_100, 48_000);
        check_resample(48_000, 44_100);
        // Sinc resampler: 44 056 : 48 000 reduces to 5 507 : 6 000.
        check_resample(44_056, 48_000);
    }

    #[test]
    fn rejects_unusable_rates_and_oversized_conversions() {
        // A hand-made WAV that claims 1 Hz: decoders accept it, the sample must not.
        let mut wav = Vec::new();
        let pcm = vec![0u8; 2 * 1_024];
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&1u32.to_le_bytes()); // 1 Hz
        wav.extend_from_slice(&2u32.to_le_bytes()); // bytes per second
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(&pcm);
        assert!(matches!(
            Sample::load(&wav, Some("wav"), 48_000, 2),
            Err(SampleError::InvalidFormat(_))
        ));

        let src = Sample::from_interleaved(vec![0.0; 1_000], 8_000, 1).unwrap();
        assert!(src.convert(500, 1).is_err());
        assert!(src.convert(48_000, 9).is_err());
        // 8 kHz mono under the limit grows 12-fold at 48 kHz stereo: over it.
        let long = Sample::from_interleaved(vec![0.0; 2_500_000], 8_000, 1).unwrap();
        assert!(matches!(long.convert(48_000, 2), Err(SampleError::TooLong)));
    }

    #[test]
    fn short_and_downsampled_lengths_are_exact() {
        // Shorter than one resampler chunk.
        let short = Sample::from_interleaved(vec![0.1; 100], 44_100, 1).unwrap();
        assert_eq!(short.convert(48_000, 1).unwrap().frames(), 109);
        let long = Sample::from_interleaved(sine(500.0, 48_000, 48_000, 0.0), 48_000, 1).unwrap();
        let down = long.convert(44_100, 1).unwrap();
        assert_eq!(down.frames(), 44_100);
        let peak = down.data()[1_000..43_000]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 1.0).abs() < 0.01, "peak {peak}");
    }

    #[test]
    fn same_format_is_a_copy() {
        let src = Sample::from_interleaved(vec![0.1, 0.2], 48_000, 1).unwrap();
        assert_eq!(src.convert(48_000, 1).unwrap(), src);
        let empty = Sample::from_interleaved(vec![], 44_100, 1).unwrap();
        assert_eq!(empty.convert(48_000, 2).unwrap().frames(), 0);
    }
}
