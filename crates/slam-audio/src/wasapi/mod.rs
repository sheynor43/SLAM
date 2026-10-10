//! WASAPI backend (ADR-0027): shared, shared low-latency (`IAudioClient3`) and exclusive
//! output on Windows.
//!
//! The COM code lives in [`com`] and builds on Windows only. This module holds the
//! platform-neutral parts — mode fallback, sample formats and conversion, period sizing,
//! latency from the audio clock — so they are tested on every platform.

#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
mod com;

#[cfg(windows)]
pub(crate) use com::{devices, open_output};

use crate::hal::WasapiMode;

/// 100-nanosecond units (`REFERENCE_TIME`) per second.
const HNS_PER_SECOND: u64 = 10_000_000;

/// Modes tried for a requested mode, in order: each falls back to the less demanding
/// ones (exclusive → low-latency shared → shared).
pub(crate) fn fallback_chain(mode: WasapiMode) -> &'static [WasapiMode] {
    use WasapiMode::*;
    match mode {
        Exclusive => &[Exclusive, LowLatency, Shared],
        LowLatency => &[LowLatency, Shared],
        Shared => &[Shared],
    }
}

/// Device sample formats, as laid out in the endpoint buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SampleFormat {
    /// 32-bit IEEE float.
    F32,
    /// 32-bit signed integer, all bits valid.
    I32,
    /// 24 valid bits left-justified in a 32-bit container (low byte zero).
    I24In32,
    /// Packed 24-bit signed integer, little-endian.
    I24,
    /// 16-bit signed integer.
    I16,
}

impl SampleFormat {
    /// Formats offered to an exclusive-mode device, most preferred first. Shared modes
    /// always use [`SampleFormat::F32`] (the audio engine converts).
    pub(crate) const EXCLUSIVE_PREFERENCE: [Self; 5] =
        [Self::F32, Self::I32, Self::I24In32, Self::I24, Self::I16];

    /// Bytes per sample in the buffer.
    pub(crate) const fn container_bytes(self) -> usize {
        match self {
            Self::F32 | Self::I32 | Self::I24In32 => 4,
            Self::I24 => 3,
            Self::I16 => 2,
        }
    }

    /// Significant bits per sample (`wValidBitsPerSample`).
    pub(crate) const fn valid_bits(self) -> u16 {
        match self {
            Self::F32 | Self::I32 => 32,
            Self::I24In32 | Self::I24 => 24,
            Self::I16 => 16,
        }
    }

    pub(crate) const fn is_float(self) -> bool {
        matches!(self, Self::F32)
    }
}

/// Converts interleaved `f32` samples to `format` in `dst`, which must hold exactly
/// `src.len() * format.container_bytes()` bytes. Samples are clamped to [-1, 1]; NaN
/// becomes silence. Allocation-free.
pub(crate) fn convert(src: &[f32], dst: &mut [u8], format: SampleFormat) {
    debug_assert_eq!(dst.len(), src.len() * format.container_bytes());
    let chunks = dst.chunks_exact_mut(format.container_bytes());
    match format {
        SampleFormat::F32 => {
            for (s, d) in src.iter().zip(chunks) {
                d.copy_from_slice(&s.to_le_bytes());
            }
        }
        SampleFormat::I32 => {
            for (s, d) in src.iter().zip(chunks) {
                // `as` saturates and maps NaN to zero.
                let v = (f64::from(s.clamp(-1.0, 1.0)) * f64::from(i32::MAX)).round() as i32;
                d.copy_from_slice(&v.to_le_bytes());
            }
        }
        SampleFormat::I24In32 => {
            for (s, d) in src.iter().zip(chunks) {
                let v = to_i24(*s) << 8;
                d.copy_from_slice(&v.to_le_bytes());
            }
        }
        SampleFormat::I24 => {
            for (s, d) in src.iter().zip(chunks) {
                d.copy_from_slice(&to_i24(*s).to_le_bytes()[..3]);
            }
        }
        SampleFormat::I16 => {
            for (s, d) in src.iter().zip(chunks) {
                let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
                d.copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}

/// `sample` as a 24-bit signed integer in the low bits of an `i32`.
fn to_i24(sample: f32) -> i32 {
    const MAX: f32 = 8_388_607.0;
    (sample.clamp(-1.0, 1.0) * MAX).round() as i32
}

/// `dwChannelMask` for `channels` interleaved channels, in the speaker order the PipeWire
/// backend uses: FL, FR, FC, LFE, BL, BR, SL, SR; mono is front centre.
pub(crate) fn channel_mask(channels: u16) -> u32 {
    // SPEAKER_* bits, in channel order.
    const ORDER: [u32; 8] = [0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x200, 0x400];
    const FRONT_CENTER: u32 = 0x4;
    match channels {
        1 => FRONT_CENTER,
        n => ORDER[..usize::from(n).min(ORDER.len())]
            .iter()
            .fold(0, |mask, bit| mask | bit),
    }
}

/// Frames in `hns` 100-ns units at `rate`, rounded to nearest.
pub(crate) fn frames_from_hns(hns: i64, rate: u32) -> u32 {
    let frames = (u128::from(hns.max(0) as u64) * u128::from(rate)
        + u128::from(HNS_PER_SECOND / 2))
        / u128::from(HNS_PER_SECOND);
    u32::try_from(frames).unwrap_or(u32::MAX)
}

/// Exclusive-mode period for `frames` at `rate`: rounded up to whole 100-ns units, at
/// least the device minimum `min_hns`.
pub(crate) fn exclusive_period_hns(frames: u32, rate: u32, min_hns: i64) -> i64 {
    let rate = u64::from(rate.max(1));
    let hns = (u64::from(frames) * HNS_PER_SECOND).div_ceil(rate);
    (hns as i64).max(min_hns)
}

/// Exclusive-mode period that matches an aligned buffer of `frames` exactly, for the retry
/// after `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` (rounded to nearest, as Microsoft's sample
/// does).
pub(crate) fn aligned_period_hns(frames: u32, rate: u32) -> i64 {
    let rate = u64::from(rate.max(1));
    ((u64::from(frames) * HNS_PER_SECOND + rate / 2) / rate) as i64
}

/// Shared-engine period limits from `IAudioClient3::GetSharedModeEnginePeriod`, frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EnginePeriods {
    pub default: u32,
    pub fundamental: u32,
    pub min: u32,
    pub max: u32,
}

/// Low-latency period for a request of `requested` frames: rounded up to a multiple of
/// the fundamental period and clamped to the engine's limits. Without a fundamental
/// period (a driver reporting zero) the default period.
pub(crate) fn low_latency_period(requested: u32, engine: EnginePeriods) -> u32 {
    if engine.fundamental == 0 || engine.min > engine.max {
        return engine.default;
    }
    let rounded = requested
        .div_ceil(engine.fundamental)
        .saturating_mul(engine.fundamental);
    rounded.clamp(engine.min, engine.max)
}

/// Frames to write on a shared-mode event: top the buffer up to two periods queued (one
/// playing, one ready), bounded by the free space. Queueing more than that only adds
/// latency.
pub(crate) fn shared_frames_to_write(buffer: u32, padding: u32, period: u32) -> u32 {
    let free = buffer.saturating_sub(padding);
    let target = period.saturating_mul(2).saturating_sub(padding);
    free.min(target)
}

/// Time until stream frame `frame` leaves the device, nanoseconds, from an
/// `IAudioClock::GetPosition` report: the device is at `position` in units of `frequency`
/// per second. Zero if the device is already past `frame`.
pub(crate) fn latency_ns(frame: u64, position: u64, frequency: u64, rate: u32) -> u64 {
    if frequency == 0 || rate == 0 {
        return 0;
    }
    // Both instants in nanoseconds of stream time, subtracted without rounding the
    // device position to whole frames first.
    let frame_ns = u128::from(frame) * 1_000_000_000 / u128::from(rate);
    let played_ns = u128::from(position) * 1_000_000_000 / u128::from(frequency);
    u64::try_from(frame_ns.saturating_sub(played_ns)).unwrap_or(u64::MAX)
}

/// `IAudioClock` QPC position (100-ns units) in engine clock nanoseconds. The engine
/// clock is `ticks · 10⁹ / frequency`; the audio clock reports `ticks · 10⁷ / frequency`,
/// so the bases agree to within 100 ns.
pub(crate) fn qpc_hns_to_ns(qpc_hns: u64) -> u64 {
    qpc_hns.saturating_mul(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_chains_end_in_shared() {
        use WasapiMode::*;
        assert_eq!(fallback_chain(Exclusive), &[Exclusive, LowLatency, Shared]);
        assert_eq!(fallback_chain(LowLatency), &[LowLatency, Shared]);
        assert_eq!(fallback_chain(Shared), &[Shared]);
    }

    fn converted(format: SampleFormat, src: &[f32]) -> Vec<u8> {
        let mut dst = vec![0xAA; src.len() * format.container_bytes()];
        convert(src, &mut dst, format);
        dst
    }

    #[test]
    fn f32_is_copied_verbatim() {
        let src = [0.25f32, -1.5, 0.0];
        let bytes = converted(SampleFormat::F32, &src);
        let back: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        assert_eq!(back, src);
    }

    #[test]
    fn i16_scales_clamps_and_silences_nan() {
        let bytes = converted(SampleFormat::I16, &[1.0, -1.0, 0.5, 2.0, -2.0, f32::NAN]);
        let v: Vec<i16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();
        assert_eq!(v, [32_767, -32_767, 16_384, 32_767, -32_767, 0]);
    }

    #[test]
    fn i32_uses_full_range() {
        let bytes = converted(SampleFormat::I32, &[1.0, -1.0, 0.0, 3.0]);
        let v: Vec<i32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes(*c))
            .collect();
        assert_eq!(v, [i32::MAX, -i32::MAX, 0, i32::MAX]);
    }

    #[test]
    fn i24_in_32_is_left_justified() {
        let bytes = converted(SampleFormat::I24In32, &[1.0, -1.0, 0.5]);
        let v: Vec<i32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes(*c))
            .collect();
        assert_eq!(v, [8_388_607 << 8, -8_388_607 << 8, 4_194_304 << 8]);
        // The padding byte is the least significant one and stays zero.
        assert!(bytes.as_chunks::<4>().0.iter().all(|c| c[0] == 0));
    }

    #[test]
    fn packed_i24_is_three_little_endian_bytes() {
        let bytes = converted(SampleFormat::I24, &[1.0, -1.0, 0.0]);
        assert_eq!(
            bytes,
            [0xFF, 0xFF, 0x7F, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn format_sizes() {
        for (format, bytes, bits) in [
            (SampleFormat::F32, 4, 32),
            (SampleFormat::I32, 4, 32),
            (SampleFormat::I24In32, 4, 24),
            (SampleFormat::I24, 3, 24),
            (SampleFormat::I16, 2, 16),
        ] {
            assert_eq!(format.container_bytes(), bytes, "{format:?}");
            assert_eq!(format.valid_bits(), bits, "{format:?}");
        }
        assert_eq!(SampleFormat::EXCLUSIVE_PREFERENCE[0], SampleFormat::F32);
    }

    #[test]
    fn channel_masks() {
        assert_eq!(channel_mask(1), 0x4);
        assert_eq!(channel_mask(2), 0x3);
        // 5.1 (back speakers) and 7.1 (back + side).
        assert_eq!(channel_mask(6), 0x3F);
        assert_eq!(channel_mask(8), 0x63F);
    }

    #[test]
    fn hns_frame_conversions() {
        // 10 ms at 48 kHz.
        assert_eq!(frames_from_hns(100_000, 48_000), 480);
        // 3 ms at 44.1 kHz = 132.3 frames.
        assert_eq!(frames_from_hns(30_000, 44_100), 132);
        assert_eq!(frames_from_hns(-5, 48_000), 0);
        // 128 frames at 48 kHz = 26666.67 hns, rounded up.
        assert_eq!(exclusive_period_hns(128, 48_000, 0), 26_667);
        // Never below the device minimum.
        assert_eq!(exclusive_period_hns(16, 48_000, 30_000), 30_000);
        // Aligned retry rounds to nearest.
        assert_eq!(aligned_period_hns(128, 48_000), 26_667);
        assert_eq!(aligned_period_hns(441, 44_100), 100_000);
    }

    #[test]
    fn low_latency_period_rounds_to_fundamental_within_limits() {
        let engine = EnginePeriods {
            default: 480,
            fundamental: 48,
            min: 48,
            max: 480,
        };
        assert_eq!(low_latency_period(128, engine), 144);
        assert_eq!(low_latency_period(96, engine), 96);
        assert_eq!(low_latency_period(16, engine), 48);
        assert_eq!(low_latency_period(4096, engine), 480);
        // A driver without small periods: min = max = default.
        let fixed = EnginePeriods {
            default: 480,
            fundamental: 480,
            min: 480,
            max: 480,
        };
        assert_eq!(low_latency_period(128, fixed), 480);
        let broken = EnginePeriods {
            fundamental: 0,
            ..engine
        };
        assert_eq!(low_latency_period(128, broken), 480);
    }

    #[test]
    fn shared_writes_keep_two_periods_queued() {
        // Empty buffer of 1056 frames, period 480: fill two periods.
        assert_eq!(shared_frames_to_write(1056, 0, 480), 960);
        // One period still queued: one more.
        assert_eq!(shared_frames_to_write(1056, 480, 480), 480);
        // Late wake-up, buffer nearly drained.
        assert_eq!(shared_frames_to_write(1056, 100, 480), 860);
        // Already two periods queued.
        assert_eq!(shared_frames_to_write(1056, 960, 480), 0);
        // Free space is the bound.
        assert_eq!(shared_frames_to_write(600, 0, 480), 600);
        assert_eq!(shared_frames_to_write(600, 700, 480), 0);
        // Minimum period the session allows.
        assert_eq!(shared_frames_to_write(100, 0, 1), 2);
        assert_eq!(shared_frames_to_write(100, 2, 1), 0);
    }

    #[test]
    fn latency_from_clock_position() {
        // Device clock in frames: 480 frames ahead at 48 kHz = 10 ms.
        assert_eq!(latency_ns(10_480, 10_000, 48_000, 48_000), 10_000_000);
        // Device clock in bytes (stereo f32: 384000 B/s), position 10000 frames.
        assert_eq!(latency_ns(10_480, 80_000, 384_000, 48_000), 10_000_000);
        // Device past the frame (or clock glitch): zero.
        assert_eq!(latency_ns(100, 10_000, 48_000, 48_000), 0);
        assert_eq!(latency_ns(100, 0, 0, 48_000), 0);
        // Device halfway through a frame: no whole-frame rounding (10 ms + 10.4 µs, each instant floored to 1 ns).
        assert_eq!(latency_ns(10_480, 19_999, 96_000, 48_000), 10_010_417);
    }

    #[test]
    fn qpc_position_converts_to_nanoseconds() {
        assert_eq!(qpc_hns_to_ns(12_345), 1_234_500);
        assert_eq!(qpc_hns_to_ns(u64::MAX), u64::MAX);
    }
}
