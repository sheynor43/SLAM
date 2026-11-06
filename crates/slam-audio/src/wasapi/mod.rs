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
pub(crate) use com::{device_rate, devices, open_output};

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

/// Events skipped at the start of an exclusive-period calibration (ADR-0101) while the
/// stream settles.
pub(crate) const CALIBRATION_WARMUP: usize = 2;

/// Event intervals measured in an exclusive-period calibration (ADR-0101); the leads are
/// taken on one event more, the last warm-up one.
pub(crate) const CALIBRATION_EVENTS: usize = 12;

/// Grid the measured event step is rounded to, 100-ns units (0.5 ms).
const PERIOD_GRID_HNS: i64 = 5_000;

/// Most exclusive periods tried at open (ADR-0101): requested → measured step → default.
pub(crate) const MAX_EXCLUSIVE_ATTEMPTS: usize = 3;

/// How far an exclusive device is ahead of the stream on a buffer event, before anything
/// is written, in stream frames: `played − written`, signed. The device is at `position`
/// in units of `frequency` per second, `written` frames handed to it so far.
///
/// On a healthy event the hardware has just moved to the last buffer written, so the lead
/// is about −buffer, offset by the thread's wake-up delay and the pipeline lag of the
/// audio clock. Both stay roughly constant. An exclusive device that finds no new buffer
/// replays the stale one and its position moves on, so every replayed buffer adds a
/// buffer to the lead for good.
pub(crate) fn lead_frames(written: u64, position: u64, frequency: u64, rate: u32) -> i64 {
    if frequency == 0 || rate == 0 {
        return 0;
    }
    let played = u128::from(position) * u128::from(rate) / u128::from(frequency);
    let played = i128::try_from(played).unwrap_or(i128::MAX);
    let lead = played - i128::from(written);
    i64::try_from(lead).unwrap_or(if lead < 0 { i64::MIN } else { i64::MAX })
}

/// Verdict of an exclusive-period calibration (ADR-0101).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CalibrationVerdict {
    /// The device keeps running ahead: the driver signals less often than the period.
    pub starving: bool,
    /// Frames the device replayed during the calibration, in whole buffers, to add to the
    /// written count so the render loop's accounting matches the device.
    pub replayed: u64,
}

/// Judges a calibration from the leads ([`lead_frames`]) of consecutive events with
/// buffers of `buffer` frames.
///
/// The ends of the window are the smallest of the first three and of the last three
/// leads: a late wake-up only raises one event's lead, a replay raises every later one.
/// Their difference is the growth over the window, with the constant offsets (wake-up
/// delay, clock lag) cancelled. One replayed buffer (a single scheduling hiccup) is
/// forgiven: the largest *persistent* step — the rise from one event to the smaller of
/// the next two — is taken off before comparing the growth with half a buffer. A lone
/// late wake-up is no persistent step, so it does not hide a driver stepping even
/// slightly slower than the period, which adds to the lead on every event.
pub(crate) fn calibration_verdict(leads: &[i64], buffer: u32) -> CalibrationVerdict {
    let none = CalibrationVerdict {
        starving: false,
        replayed: 0,
    };
    if leads.len() < 4 || buffer == 0 {
        return none;
    }
    let buffer = i128::from(buffer);
    let lead = |i: usize| i128::from(leads[i]);
    let ends = leads.len().min(6) / 2;
    let first = (0..ends).map(lead).min().unwrap_or(0);
    let last = (leads.len() - ends..leads.len())
        .map(lead)
        .min()
        .unwrap_or(0);
    let growth = last - first;
    let largest_step = (1..leads.len() - 1)
        .map(|k| lead(k).min(lead(k + 1)) - lead(k - 1))
        .max()
        .unwrap_or(0)
        .max(0);
    let replayed = if growth > 0 {
        (growth + buffer / 2) / buffer * buffer
    } else {
        0
    };
    CalibrationVerdict {
        starving: 2 * (growth - largest_step) >= buffer,
        replayed: u64::try_from(replayed).unwrap_or(u64::MAX),
    }
}

/// Median of `intervals` (the upper one for an even count); sorts in place. Zero if empty.
pub(crate) fn median_ns(intervals: &mut [u64]) -> u64 {
    intervals.sort_unstable();
    intervals.get(intervals.len() / 2).copied().unwrap_or(0)
}

/// Whether an exclusive stream with period `current_hns` needs a calibration: periods
/// below the device default may be shorter than the driver's real event step; a reopened
/// stream is always checked again.
pub(crate) fn calibrates(current_hns: i64, default_hns: i64, reopened: bool) -> bool {
    reopened || current_hns < default_hns
}

/// Period to reopen a starving exclusive stream with (ADR-0101), or `None` when the
/// stream already runs at the device default or longer and nothing is left to try.
///
/// The candidate is the measured event step `step_ns` rounded up to 0.5 ms; if that is
/// not longer than `current_hns`, the next 0.5 ms step above it. A candidate at
/// or past the default period becomes the default period itself.
pub(crate) fn next_exclusive_period_hns(
    current_hns: i64,
    default_hns: i64,
    step_ns: u64,
) -> Option<i64> {
    if current_hns >= default_hns {
        return None;
    }
    let grid = PERIOD_GRID_HNS as u64;
    let rounded = step_ns.div_ceil(100).div_ceil(grid).saturating_mul(grid);
    let rounded = i64::try_from(rounded).unwrap_or(i64::MAX);
    let candidate = if rounded > current_hns {
        rounded
    } else {
        (current_hns / PERIOD_GRID_HNS + 1) * PERIOD_GRID_HNS
    };
    Some(candidate.min(default_hns))
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

/// Whether the device, at `position` in units of `frequency` per second, has consumed every
/// one of the `written` stream frames: it ran dry before this callback. A healthy stream
/// always has at least one buffer queued when its event fires. Some exclusive-mode drivers
/// accept a period shorter than the one they signal at and then starve on every event.
///
/// In exclusive mode the position is what the device played and runs past `written` when
/// it starves; in shared mode it is what the engine took from the buffer and stops exactly
/// at `written`, hence `>=`.
pub(crate) fn ran_dry(written: u64, position: u64, frequency: u64, rate: u32) -> bool {
    if frequency == 0 || rate == 0 {
        return false;
    }
    // played / frequency >= written / rate, without rounding either side.
    u128::from(position) * u128::from(rate) >= u128::from(written) * u128::from(frequency)
}

/// Whether a callback finding the device at `position` counts as an underrun. Before any
/// audio follows the silent pre-`Start` fill (`written == prefill`) a drained device has
/// only played silence: the shared engine pulls the whole fill at once, so the first event
/// always sees it drained.
pub(crate) fn underrun(
    written: u64,
    prefill: u64,
    position: u64,
    frequency: u64,
    rate: u32,
) -> bool {
    written > prefill && ran_dry(written, position, frequency, rate)
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
    fn lead_is_signed_and_clock_unit_independent() {
        // Healthy event: the device just started the last 144-frame buffer.
        assert_eq!(lead_frames(1_440, 1_296, 48_000, 48_000), -144);
        // The device at the written end and past it.
        assert_eq!(lead_frames(1_440, 1_440, 48_000, 48_000), 0);
        assert_eq!(lead_frames(1_440, 1_776, 48_000, 48_000), 336);
        // Clock in bytes (stereo f32, 384000 B/s).
        assert_eq!(lead_frames(1_440, 1_296 * 8, 384_000, 48_000), -144);
        // No clock or rate: nothing known.
        assert_eq!(lead_frames(1_440, 1_440, 0, 48_000), 0);
        assert_eq!(lead_frames(1_440, 1_440, 48_000, 0), 0);
        // Saturates at the extremes.
        assert_eq!(lead_frames(0, u64::MAX, 1, u32::MAX), i64::MAX);
        assert_eq!(lead_frames(u64::MAX, 0, 1, 1), i64::MIN);
    }

    /// Leads of `events` events where the device plays `step` frames per event and gets
    /// `buffer`, starting from a healthy `-buffer - lag`.
    fn drifting(events: usize, buffer: i64, step: i64, lag: i64) -> Vec<i64> {
        (0..events as i64)
            .map(|i| -buffer - lag + i * (step - buffer))
            .collect()
    }

    #[test]
    fn healthy_calibration_is_accepted_whatever_the_clock_lag() {
        for lag in [0, 40, 100, 140] {
            let leads = drifting(13, 144, 144, lag);
            assert_eq!(
                calibration_verdict(&leads, 144),
                CalibrationVerdict {
                    starving: false,
                    replayed: 0
                },
                "lag {lag}"
            );
        }
    }

    #[test]
    fn ur12_at_3_ms_starves() {
        // 480 frames played per 144 written.
        let verdict = calibration_verdict(&drifting(13, 144, 480, 30), 144);
        assert!(verdict.starving);
    }

    #[test]
    fn a_step_slightly_longer_than_the_period_starves() {
        // 5 ms requested (240 frames), the driver steps at 256 frames.
        let verdict = calibration_verdict(&drifting(13, 240, 256, 60), 240);
        assert!(verdict.starving);
    }

    #[test]
    fn a_late_wake_up_does_not_hide_a_slow_driver() {
        // 5.33 ms against 5 ms, plus one wake-up 1 ms (48 frames) late mid-window.
        let mut leads = drifting(13, 240, 256, 60);
        leads[6] += 48;
        assert!(calibration_verdict(&leads, 240).starving);
    }

    #[test]
    fn one_replay_is_forgiven_and_realigned() {
        let mut leads = vec![-150i64; 13];
        // A late wake-up on event 6 made the device replay one buffer: +144 from then on.
        for lead in &mut leads[6..] {
            *lead += 144;
        }
        assert_eq!(
            calibration_verdict(&leads, 144),
            CalibrationVerdict {
                starving: false,
                replayed: 144
            }
        );
    }

    #[test]
    fn a_late_last_event_does_not_shift_the_count() {
        let mut leads = vec![-150i64; 13];
        // The thread woke 2.5 ms late on the last event, but nothing was replayed.
        leads[12] += 120;
        assert_eq!(
            calibration_verdict(&leads, 144),
            CalibrationVerdict {
                starving: false,
                replayed: 0
            }
        );
        // Two late wake-ups in a row at the end.
        let mut leads = vec![-150i64; 13];
        leads[11] += 120;
        leads[12] += 100;
        assert_eq!(calibration_verdict(&leads, 144).replayed, 0);
        // Same at the start.
        let mut leads = vec![-150i64; 13];
        leads[0] += 120;
        leads[1] += 100;
        assert_eq!(calibration_verdict(&leads, 144).replayed, 0);
    }

    #[test]
    fn wake_up_jitter_alone_is_not_starving() {
        let jitter = [0, 30, -10, 50, 0, 70, -20, 10, 60, 0, 40, -30, 20];
        let leads: Vec<i64> = jitter.iter().map(|j| -150 + j).collect();
        assert_eq!(
            calibration_verdict(&leads, 144),
            CalibrationVerdict {
                starving: false,
                replayed: 0
            }
        );
    }

    #[test]
    fn too_short_a_calibration_has_no_verdict() {
        let none = CalibrationVerdict {
            starving: false,
            replayed: 0,
        };
        assert_eq!(calibration_verdict(&[0, 500, 1_000], 144), none);
        assert_eq!(calibration_verdict(&drifting(13, 144, 480, 0), 0), none);
    }

    #[test]
    fn median_ignores_outliers() {
        let mut intervals = [10_010_000, 9_990_000, 30_000_000, 10_000_000, 1_000];
        assert_eq!(median_ns(&mut intervals), 10_000_000);
        let mut even = [4, 1, 3, 2];
        assert_eq!(median_ns(&mut even), 3);
        assert_eq!(median_ns(&mut []), 0);
    }

    #[test]
    fn only_short_or_reopened_periods_calibrate() {
        // 3 ms requested, device default 10 ms.
        assert!(calibrates(30_000, 100_000, false));
        assert!(!calibrates(100_000, 100_000, false));
        assert!(!calibrates(200_000, 100_000, false));
        // After a reopen at the default period: checked once more.
        assert!(calibrates(100_000, 100_000, true));
    }

    #[test]
    fn next_period_follows_the_measured_step() {
        // UR12: 3 ms requested, events every ~10 ms, default 10 ms.
        assert_eq!(
            next_exclusive_period_hns(30_000, 100_000, 10_020_000),
            Some(100_000)
        );
        // A driver stepping at 5.33 ms (256 frames at 48 kHz): rounded up to 5.5 ms.
        assert_eq!(
            next_exclusive_period_hns(30_000, 100_000, 5_333_333),
            Some(55_000)
        );
        // Never below the step: 4.1 ms becomes 4.5 ms, an exact 4 ms stays.
        assert_eq!(
            next_exclusive_period_hns(30_000, 100_000, 4_100_000),
            Some(45_000)
        );
        assert_eq!(
            next_exclusive_period_hns(30_000, 100_000, 4_000_000),
            Some(40_000)
        );
        // Step that rounds to the current period or below: the next 0.5 ms above it.
        assert_eq!(
            next_exclusive_period_hns(30_000, 100_000, 3_200_000),
            Some(35_000)
        );
        assert_eq!(
            next_exclusive_period_hns(26_667, 100_000, 2_000_000),
            Some(30_000)
        );
        // Step at or past the default: the default.
        assert_eq!(
            next_exclusive_period_hns(55_000, 100_000, 12_000_000),
            Some(100_000)
        );
        assert_eq!(
            next_exclusive_period_hns(97_000, 100_000, 9_800_000),
            Some(100_000)
        );
        // Already at the default: nothing left to try.
        assert_eq!(
            next_exclusive_period_hns(100_000, 100_000, 20_000_000),
            None
        );
        assert_eq!(
            next_exclusive_period_hns(150_000, 100_000, 20_000_000),
            None
        );
        // A huge step saturates instead of overflowing.
        assert_eq!(
            next_exclusive_period_hns(30_000, 100_000, u64::MAX),
            Some(100_000)
        );
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
    fn ran_dry_when_the_device_played_everything_written() {
        // One 480-frame buffer still queued: healthy.
        assert!(!ran_dry(10_480, 10_000, 48_000, 48_000));
        // A single frame queued is still not dry.
        assert!(!ran_dry(10_001, 10_000, 48_000, 48_000));
        // Device exactly at or past the written end: it starved.
        assert!(ran_dry(10_000, 10_000, 48_000, 48_000));
        assert!(ran_dry(10_000, 13_000, 48_000, 48_000));
        // Clock in bytes (stereo f32: 384000 B/s): 10000 frames played of 10480 / 10000.
        assert!(!ran_dry(10_480, 80_000, 384_000, 48_000));
        assert!(ran_dry(10_000, 80_000, 384_000, 48_000));
        // Half a frame short of the end (clock at 96 kHz): not yet.
        assert!(!ran_dry(10_000, 19_999, 96_000, 48_000));
        // No clock frequency or rate: no verdict.
        assert!(!ran_dry(0, 10_000, 0, 48_000));
        assert!(!ran_dry(0, 10_000, 48_000, 0));
        // No overflow at the extremes.
        assert!(ran_dry(u64::MAX, u64::MAX, u64::from(u32::MAX), u32::MAX));
        assert!(!ran_dry(
            u64::MAX,
            u64::MAX - 1,
            u64::from(u32::MAX),
            u32::MAX
        ));
    }

    #[test]
    fn underrun_ignores_the_silent_prefill() {
        // Shared start: 960 silent frames written, the engine took them all (7680 B at
        // 384000 B/s). Not an underrun: only silence was due.
        assert!(!underrun(960, 960, 7_680, 384_000, 48_000));
        // The next callback drained as well: a real gap.
        assert!(underrun(1_440, 960, 11_520, 384_000, 48_000));
        // Healthy after the prefill: a buffer still queued.
        assert!(!underrun(1_440, 960, 7_680, 384_000, 48_000));
        // No prefill (`frames_due` was 0 at start): the first written audio counts.
        assert!(underrun(480, 0, 480, 48_000, 48_000));
        assert!(!underrun(0, 0, 0, 48_000, 48_000));
        // Exclusive after a failed `GetBuffer` (frames counted as written anyway): the
        // device replaying the stale buffer is not reported twice, only once caught up.
        assert!(!underrun(1_008, 144, 864, 48_000, 48_000));
        assert!(underrun(1_008, 144, 1_008, 48_000, 48_000));
    }

    #[test]
    fn qpc_position_converts_to_nanoseconds() {
        assert_eq!(qpc_hns_to_ns(12_345), 1_234_500);
        assert_eq!(qpc_hns_to_ns(u64::MAX), u64::MAX);
    }
}
