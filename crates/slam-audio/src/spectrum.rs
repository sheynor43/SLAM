//! The music's spectrum for visualisers (ADR-0186): the decoder thread measures every
//! block it renders and publishes the magnitudes, labelled with the block's track frames,
//! in a ring of seqlocked slots; the update thread reads the slot the track is playing.
//! The audio callback is not involved.
//!
//! The magnitudes are like lazer's `Track.CurrentAmplitudes.FrequencyAmplitudes` (BASS
//! `FFT512`: 256 bins from 0 Hz to Nyquist, a full-scale sine near 1): a Hann-windowed
//! FFT of [`FFT_SIZE`] mono frames, the larger of each pair of adjacent bins.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::seqlock::SeqLock;

/// Bins published per block.
pub const SPECTRUM_BINS: usize = 256;
/// Frames measured per block (from its start; a shorter block is zero-padded).
pub const FFT_SIZE: usize = 1024;
/// Blocks remembered by default; a music stream takes twice its pool of blocks.
const SLOTS: usize = 32;
/// Header words: track frame of the first frame, track frames covered, generation.
const HEADER: usize = 3;
/// Two `f32` magnitudes per word.
const WORDS: usize = HEADER + SPECTRUM_BINS / 2;

/// The published spectra of one music stream. Shared by the decoder (writer) and the
/// update thread (reader); neither waits for the other.
#[derive(Debug)]
pub struct Spectrum {
    slots: Box<[SeqLock<WORDS>]>,
    /// Generation of each slot, also kept outside the lock so that the reader skips the
    /// slots of other tracks without copying them.
    generations: Box<[AtomicU64]>,
}

impl Default for Spectrum {
    fn default() -> Self {
        Spectrum::with_slots(SLOTS)
    }
}

impl Spectrum {
    /// Room for the spectra of `slots` blocks (at least one): more than the blocks the
    /// decoder can run ahead, or the heard one's is overwritten before it is read.
    pub(crate) fn with_slots(slots: usize) -> Spectrum {
        let slots = slots.max(1);
        Spectrum {
            slots: (0..slots).map(|_| SeqLock::default()).collect(),
            generations: (0..slots).map(|_| AtomicU64::new(u64::MAX)).collect(),
        }
    }

    /// Publishes `bins` of the block numbered `seq` covering track frames `start` to
    /// `start + frames`, of `generation`. The bins describe the block's first
    /// [`FFT_SIZE`] frames (the whole of a default block).
    pub(crate) fn publish(
        &self,
        seq: u32,
        generation: u32,
        start: f64,
        frames: f64,
        bins: &[f32; SPECTRUM_BINS],
    ) {
        let index = seq as usize % self.slots.len();
        let mut words = [0u64; WORDS];
        words[0] = start.to_bits();
        words[1] = frames.to_bits();
        words[2] = u64::from(generation);
        for (word, pair) in words[HEADER..].iter_mut().zip(bins.as_chunks::<2>().0) {
            *word = u64::from(pair[0].to_bits()) | u64::from(pair[1].to_bits()) << 32;
        }
        self.generations[index].store(u64::MAX, Ordering::Relaxed);
        self.slots[index].write(words);
        self.generations[index].store(u64::from(generation), Ordering::Release);
    }

    /// Copies into `out` the magnitudes of the block of `generation` that holds track
    /// frame `frame`; false (and `out` untouched) when none does. Does not allocate, and
    /// never waits for the decoder: a slot being written is skipped.
    pub fn read(&self, generation: u32, frame: f64, out: &mut [f32; SPECTRUM_BINS]) -> bool {
        for (slot, slot_generation) in self.slots.iter().zip(self.generations.iter()) {
            if slot_generation.load(Ordering::Acquire) != u64::from(generation) {
                continue;
            }
            let Some(words) = slot.try_read() else {
                continue;
            };
            let start = f64::from_bits(words[0]);
            let frames = f64::from_bits(words[1]);
            if words[2] != u64::from(generation) || !(start <= frame && frame < start + frames) {
                continue;
            }
            for (pair, word) in out.as_chunks_mut::<2>().0.iter_mut().zip(&words[HEADER..]) {
                pair[0] = f32::from_bits(*word as u32);
                pair[1] = f32::from_bits((*word >> 32) as u32);
            }
            return true;
        }
        false
    }
}

/// The decoder's measuring tool: window, twiddles and scratch made once.
pub(crate) struct Analyser {
    window: Box<[f32]>,
    /// `cos` and `sin` of `-2πk/N` for k below N/2.
    twiddles: Box<[(f32, f32)]>,
    re: Box<[f32]>,
    im: Box<[f32]>,
    bins: [f32; SPECTRUM_BINS],
}

impl Analyser {
    pub(crate) fn new() -> Analyser {
        let n = FFT_SIZE;
        let window: Box<[f32]> = (0..n)
            .map(|i| {
                let phase = std::f64::consts::TAU * i as f64 / n as f64;
                (0.5 - 0.5 * phase.cos()) as f32
            })
            .collect();
        let twiddles = (0..n / 2)
            .map(|k| {
                let angle = -std::f64::consts::TAU * k as f64 / n as f64;
                (angle.cos() as f32, angle.sin() as f32)
            })
            .collect();
        Analyser {
            window,
            twiddles,
            re: vec![0.0; n].into_boxed_slice(),
            im: vec![0.0; n].into_boxed_slice(),
            bins: [0.0; SPECTRUM_BINS],
        }
    }

    /// Measures the first [`FFT_SIZE`] frames of `data` (`channels` interleaved, `frames`
    /// of them). Does not allocate.
    pub(crate) fn measure(
        &mut self,
        data: &[f32],
        channels: usize,
        frames: usize,
    ) -> &[f32; SPECTRUM_BINS] {
        let n = FFT_SIZE;
        let channels = channels.max(1);
        let used = frames.min(n).min(data.len() / channels);
        let inv = 1.0 / channels as f32;
        for i in 0..n {
            let mono = if i < used {
                data[i * channels..(i + 1) * channels].iter().sum::<f32>() * inv
            } else {
                0.0
            };
            self.re[i] = mono * self.window[i];
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im, &self.twiddles);
        // A full-scale sine peaks at N/4 under the Hann window (coherent gain 1/2, half
        // the energy in the positive bin): scale so that it reads 1, as BASS's FFT does.
        // Unlike BASS, louder bins are capped at 1 (the visualiser's bars stay bounded).
        let scale = 4.0 / n as f32;
        for (k, bin) in self.bins.iter_mut().enumerate() {
            let magnitude = |j: usize| (self.re[j] * self.re[j] + self.im[j] * self.im[j]).sqrt();
            *bin = (magnitude(2 * k).max(magnitude(2 * k + 1)) * scale).min(1.0);
        }
        &self.bins
    }
}

/// In-place iterative radix-2 FFT of `re` + i·`im` (length a power of two, twiddles for
/// it).
fn fft(re: &mut [f32], im: &mut [f32], twiddles: &[(f32, f32)]) {
    let n = re.len();
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let stride = n / len;
        for start in (0..n).step_by(len) {
            for k in 0..half {
                let (c, s) = twiddles[k * stride];
                let (a, b) = (start + k, start + k + half);
                let tr = re[b] * c - im[b] * s;
                let ti = re[b] * s + im[b] * c;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency_bin: f64, amplitude: f32, channels: usize) -> Vec<f32> {
        (0..FFT_SIZE)
            .flat_map(|i| {
                let v = amplitude
                    * (std::f64::consts::TAU * frequency_bin * i as f64 / FFT_SIZE as f64).sin()
                        as f32;
                std::iter::repeat_n(v, channels)
            })
            .collect()
    }

    #[test]
    fn a_full_scale_sine_reads_one_in_its_bin() {
        let mut analyser = Analyser::new();
        // FFT bin 100 is published bin 50.
        let bins = *analyser.measure(&sine(100.0, 1.0, 2), 2, FFT_SIZE);
        assert!((bins[50] - 1.0).abs() < 0.05, "{}", bins[50]);
        assert!(bins[10] < 0.01 && bins[200] < 0.01);
        let quiet = *analyser.measure(&sine(100.0, 0.25, 1), 1, FFT_SIZE);
        assert!((quiet[50] - 0.25).abs() < 0.02, "{}", quiet[50]);
    }

    #[test]
    fn silence_and_short_blocks() {
        let mut analyser = Analyser::new();
        assert!(
            analyser
                .measure(&[0.0; 64], 2, 32)
                .iter()
                .all(|&b| b == 0.0)
        );
        // A block shorter than the window is zero-padded, not read past.
        let short = sine(64.0, 1.0, 1);
        let bins = analyser.measure(&short[..100], 1, 100);
        assert!(bins.iter().all(|b| b.is_finite()));
    }

    #[test]
    fn the_reader_finds_the_block_of_its_generation_and_frame() {
        let spectrum = Spectrum::default();
        let mut bins = [0.0; SPECTRUM_BINS];
        let mut out = [9.0; SPECTRUM_BINS];
        assert!(!spectrum.read(1, 0.0, &mut out));
        bins[3] = 0.5;
        spectrum.publish(0, 1, 0.0, 1024.0, &bins);
        bins[3] = 0.75;
        spectrum.publish(1, 1, 1024.0, 1024.0, &bins);
        assert!(spectrum.read(1, 1500.0, &mut out));
        assert_eq!(out[3], 0.75);
        assert!(spectrum.read(1, 10.0, &mut out));
        assert_eq!(out[3], 0.5);
        // Another track's generation, or a frame not decoded, finds nothing.
        assert!(!spectrum.read(2, 10.0, &mut out));
        assert!(!spectrum.read(1, 4096.0, &mut out));
        // A slot is reused after SLOTS blocks.
        spectrum.publish(SLOTS as u32, 2, 0.0, 1024.0, &bins);
        assert!(!spectrum.read(1, 10.0, &mut out));
        assert!(spectrum.read(2, 10.0, &mut out));
    }
}
