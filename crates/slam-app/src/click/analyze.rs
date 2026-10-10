//! Click-to-sound analysis of a microphone recording: finds the mechanical click of
//! the mouse button and the onset of the test tone from the speaker, pairs them and
//! reports the physical latency. Developer tool: console output is not localised.
//! Method: `docs/perf/2026-10-10-click-to-sound.md`.

use std::fmt::Write as _;
use std::path::PathBuf;

use slam_audio::Sample;

use super::tone_burst;
use crate::bench::number;

pub const USAGE: &str = "usage: slam-app click-analyze FILE.wav [--tone-hz HZ] [--window-ms MS] \
                         [--dump FILE.csv]";

/// Analysis sample rate: recordings are resampled to it on load.
pub const SAMPLE_RATE: u32 = 48_000;

/// Smoothing window of the click envelope.
const CLICK_ENV_MS: f64 = 0.25;
/// Click threshold: this many times the median envelope ...
const CLICK_FLOOR_FACTOR: f32 = 8.0;
/// ... or this fraction of the loudest click, whichever is higher.
const CLICK_PEAK_FRACTION: f32 = 0.05;
/// The onset is walked back to where the envelope rose above this many floors.
const CLICK_ONSET_FLOOR_FACTOR: f32 = 3.0;
/// Presses closer than this are one click.
const CLICK_REFRACTORY_S: f64 = 0.030;
/// The raw high-passed signal must exceed this many median magnitudes for the onset.
const CLICK_RAW_FACTOR: f32 = 6.0;

/// Tone detector threshold: this many times the median band envelope.
const TONE_FLOOR_FACTOR: f32 = 10.0;
/// Absolute minimum band amplitude (about -100 dBFS).
const TONE_MIN_LEVEL: f32 = 1e-5;
/// The tone must stay above the threshold this long (clicks decay sooner).
const TONE_SUSTAIN_S: f64 = 0.008;
/// The onset is where the band envelope crosses this fraction of its peak.
const TONE_ONSET_FRACTION: f32 = 0.25;
const TONE_REFRACTORY_S: f64 = 0.050;
/// Band envelope window: whole cycles, at least this long.
const TONE_WINDOW_S: f64 = 0.001;

/// Earliest gap between a click and the tone it is paired with.
const MIN_LATENCY_S: f64 = 0.0005;
/// A click this close after the previous tone's onset still belongs to that tone.
const PAIR_GUARD_S: f64 = 0.030;

/// `click-analyze` settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub file: PathBuf,
    pub tone_hz: f64,
    /// How long before a tone its click is searched for.
    pub window_ms: f64,
    pub dump: Option<PathBuf>,
}

impl Options {
    /// Parses the arguments after `click-analyze`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut file = None;
        let mut tone_hz = 1000.0;
        let mut window_ms = 100.0;
        let mut dump = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--tone-hz" => tone_hz = number(&arg, &value()?)?,
                "--window-ms" => window_ms = number(&arg, &value()?)?,
                "--dump" => dump = Some(PathBuf::from(value()?)),
                _ if arg.starts_with("--") => return Err(format!("unknown argument {arg}")),
                _ if file.is_none() => file = Some(PathBuf::from(arg)),
                _ => return Err(format!("unexpected argument {arg}")),
            }
        }
        let nyquist = f64::from(SAMPLE_RATE) / 2.0;
        if !(tone_hz > 0.0 && tone_hz < nyquist) {
            return Err(format!("--tone-hz must be in (0, {nyquist})"));
        }
        if !(window_ms > 0.5 && window_ms <= 10_000.0) {
            return Err("--window-ms must be in (0.5, 10000]".to_owned());
        }
        Ok(Self {
            file: file.ok_or("no recording given")?,
            tone_hz,
            window_ms,
            dump,
        })
    }
}

/// One matched click and tone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub click_s: f64,
    pub tone_s: f64,
}

impl Pair {
    pub fn latency_ms(&self) -> f64 {
        (self.tone_s - self.click_s) * 1000.0
    }
}

/// Everything found in a recording; times are seconds from its start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub duration_s: f64,
    pub clicks: Vec<f64>,
    pub tones: Vec<f64>,
    pub pairs: Vec<Pair>,
}

/// Latency statistics in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub min: f64,
    pub p10: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
    pub stddev: f64,
}

impl Stats {
    /// `None` for no values.
    pub fn of(values: &[f64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        // Nearest rank: the smallest value with at least p percent of values at or below it.
        let rank = |p: f64| sorted[(((p / 100.0) * n as f64).ceil() as usize).clamp(1, n) - 1];
        let mean = sorted.iter().sum::<f64>() / n as f64;
        let variance = sorted.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
        Some(Self {
            min: sorted[0],
            p10: rank(10.0),
            p50: rank(50.0),
            p90: rank(90.0),
            p99: rank(99.0),
            max: sorted[n - 1],
            mean,
            stddev: variance.sqrt(),
        })
    }
}

/// Finds clicks, tones and their pairs in a mono recording.
pub fn analyze(signal: &[f32], rate: u32, tone_hz: f64, window_ms: f64) -> Analysis {
    let clicks = click_onsets(signal, rate, tone_hz);
    let bias = tone_bias_s(tone_hz, rate);
    let tones: Vec<f64> = tone_onsets(signal, rate, tone_hz)
        .into_iter()
        .map(|t| t - bias)
        .collect();
    let pairs = pair(&clicks, &tones, window_ms / 1000.0);
    Analysis {
        duration_s: signal.len() as f64 / f64::from(rate),
        clicks,
        tones,
        pairs,
    }
}

/// For each tone the earliest click in `[tone - window, tone - 0.5 ms]` that is later
/// than the previous tone plus 30 ms.
pub fn pair(clicks: &[f64], tones: &[f64], window_s: f64) -> Vec<Pair> {
    let mut pairs = Vec::new();
    let mut previous = f64::NEG_INFINITY;
    for &tone_s in tones {
        let from = (tone_s - window_s).max(previous + PAIR_GUARD_S);
        let to = tone_s - MIN_LATENCY_S;
        if let Some(&click_s) = clicks.iter().find(|&&c| c >= from && c <= to) {
            pairs.push(Pair { click_s, tone_s });
        }
        previous = tone_s;
    }
    pairs
}

/// Seconds of the mechanical click onsets.
pub fn click_onsets(signal: &[f32], rate: u32, tone_hz: f64) -> Vec<f64> {
    let rate_hz = f64::from(rate);
    // High enough that the tone does not register, low enough to keep the click.
    let cutoff = (3.0 * tone_hz).clamp(4000.0, 0.4 * rate_hz);
    let filtered = zero_phase(signal, Biquad::high_pass(cutoff, rate_hz));
    let window = ((CLICK_ENV_MS / 1000.0 * rate_hz).round() as usize).max(1);
    let magnitude: Vec<f32> = filtered.iter().map(|v| v.abs()).collect();
    let envelope = centered_mean(&magnitude, window);
    let floor = median(&envelope);
    let raw_floor = median(&magnitude);
    let peak = envelope.iter().fold(0.0f32, |m, &v| m.max(v));
    let threshold = (CLICK_FLOOR_FACTOR * floor)
        .max(CLICK_PEAK_FRACTION * peak)
        .max(1e-6);
    let onset_level = CLICK_ONSET_FLOOR_FACTOR * floor;
    let refractory = (CLICK_REFRACTORY_S * rate_hz) as usize;

    let mut onsets = Vec::new();
    let mut i = 0;
    while i < envelope.len() {
        if envelope[i] <= threshold {
            i += 1;
            continue;
        }
        let mut start = i;
        while start > 0 && envelope[start - 1] > onset_level {
            start -= 1;
        }
        // The smoothing window sees the burst early: take the first strong raw sample.
        let strong = CLICK_RAW_FACTOR * raw_floor;
        let end = (start + 2 * window).min(magnitude.len());
        let onset = (start..end)
            .find(|&k| magnitude[k] > strong)
            .unwrap_or(start + window / 2);
        onsets.push(onset as f64 / rate_hz);
        i = (onset + refractory).max(i + 1);
    }
    onsets
}

/// Seconds of the tone onsets, as seen by the band envelope (see [`tone_bias_s`]).
pub fn tone_onsets(signal: &[f32], rate: u32, tone_hz: f64) -> Vec<f64> {
    let rate_hz = f64::from(rate);
    let envelope = tone_envelope(signal, rate, tone_hz);
    let floor = median(&envelope);
    let threshold = (TONE_FLOOR_FACTOR * floor).max(TONE_MIN_LEVEL);
    let sustain = (TONE_SUSTAIN_S * rate_hz) as usize;
    let refractory = (TONE_REFRACTORY_S * rate_hz) as usize;

    let mut onsets = Vec::new();
    let mut i = 0;
    while i < envelope.len() {
        if envelope[i] <= threshold {
            i += 1;
            continue;
        }
        let end = envelope[i..]
            .iter()
            .position(|&v| v <= threshold)
            .map_or(envelope.len(), |n| i + n);
        if end - i < sustain {
            i = end;
            continue;
        }
        let onset = envelope_onset(&envelope, i, end);
        onsets.push(onset as f64 / rate_hz);
        i = end.max(onset + refractory);
    }
    onsets
}

/// Where the band envelope crosses a fraction of the peak of the run `[from, to)`.
fn envelope_onset(envelope: &[f32], from: usize, to: usize) -> usize {
    let mut peak = from;
    for k in from..to {
        if envelope[k] > envelope[peak] {
            peak = k;
        }
    }
    let level = TONE_ONSET_FRACTION * envelope[peak];
    let mut onset = peak;
    while onset > 0 && envelope[onset - 1] >= level {
        onset -= 1;
    }
    onset
}

/// How late the band envelope reports the start of the generated burst. Measured on the
/// burst itself, so that the report is of the burst's first sample.
pub fn tone_bias_s(tone_hz: f64, rate: u32) -> f64 {
    let pad = rate as usize / 50;
    let burst = tone_burst(rate, 1, tone_hz, 30.0);
    let mut signal = vec![0.0f32; pad];
    signal.extend_from_slice(burst.data());
    signal.resize(signal.len() + pad, 0.0);
    match tone_onsets(&signal, rate, tone_hz).first() {
        Some(&t) => t - pad as f64 / f64::from(rate),
        None => 0.0,
    }
}

/// Amplitude of the signal at `tone_hz`, quadrature demodulated over whole cycles.
fn tone_envelope(signal: &[f32], rate: u32, tone_hz: f64) -> Vec<f32> {
    let rate_hz = f64::from(rate);
    let cycles = (tone_hz * TONE_WINDOW_S).ceil().max(1.0);
    let window = ((cycles * rate_hz / tone_hz).round() as usize).max(2);
    let step = std::f64::consts::TAU * tone_hz / rate_hz;
    let mut cos_sum = Vec::with_capacity(signal.len() + 1);
    let mut sin_sum = Vec::with_capacity(signal.len() + 1);
    let (mut c, mut s) = (0.0f64, 0.0f64);
    cos_sum.push(c);
    sin_sum.push(s);
    for (i, &v) in signal.iter().enumerate() {
        let (sin, cos) = (step * i as f64).sin_cos();
        c += f64::from(v) * cos;
        s += f64::from(v) * sin;
        cos_sum.push(c);
        sin_sum.push(s);
    }
    let n = signal.len();
    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(window / 2);
            let hi = (i + window - window / 2).min(n);
            let (dc, ds) = (cos_sum[hi] - cos_sum[lo], sin_sum[hi] - sin_sum[lo]);
            (2.0 * dc.hypot(ds) / window as f64) as f32
        })
        .collect()
}

/// Mean over a window centred on each sample (shorter at the edges).
fn centered_mean(values: &[f32], window: usize) -> Vec<f32> {
    let mut sums = Vec::with_capacity(values.len() + 1);
    let mut total = 0.0f64;
    sums.push(total);
    for &v in values {
        total += f64::from(v);
        sums.push(total);
    }
    let n = values.len();
    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(window / 2);
            let hi = (i + window - window / 2).min(n);
            ((sums[hi] - sums[lo]) / (hi - lo) as f64) as f32
        })
        .collect()
}

fn median(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut copy = values.to_vec();
    let mid = copy.len() / 2;
    *copy.select_nth_unstable_by(mid, f32::total_cmp).1
}

/// Second-order section, direct form I.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
}

impl Biquad {
    /// RBJ high-pass, Butterworth Q.
    fn high_pass(cutoff: f64, rate: f64) -> Self {
        let w = std::f64::consts::TAU * cutoff / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        Self {
            b: [
                (1.0 + cos) / 2.0 / a0,
                -(1.0 + cos) / a0,
                (1.0 + cos) / 2.0 / a0,
            ],
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
        }
    }

    fn run<'a>(&self, input: impl Iterator<Item = &'a f64>) -> Vec<f64> {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        input
            .map(|&x| {
                let y = self.b[0] * x + self.b[1] * x1 + self.b[2] * x2
                    - self.a[0] * y1
                    - self.a[1] * y2;
                (x2, x1, y2, y1) = (x1, x, y1, y);
                y
            })
            .collect()
    }
}

/// Forward-backward filtering: no phase shift, so onsets keep their place.
fn zero_phase(signal: &[f32], filter: Biquad) -> Vec<f32> {
    let wide: Vec<f64> = signal.iter().map(|&v| f64::from(v)).collect();
    let forward = filter.run(wide.iter());
    let mut backward = filter.run(forward.iter().rev());
    backward.reverse();
    backward.into_iter().map(|v| v as f32).collect()
}

/// Human-readable report.
pub fn report(file: &str, analysis: &Analysis) -> String {
    let mut out = String::new();
    // Writing to a String cannot fail.
    let _ = writeln!(
        out,
        "{file}: {:.1} s, {} clicks, {} tones, {} pairs ({} clicks and {} tones unpaired)",
        analysis.duration_s,
        analysis.clicks.len(),
        analysis.tones.len(),
        analysis.pairs.len(),
        analysis.clicks.len() - analysis.pairs.len(),
        analysis.tones.len() - analysis.pairs.len(),
    );
    let latencies: Vec<f64> = analysis.pairs.iter().map(Pair::latency_ms).collect();
    match Stats::of(&latencies) {
        None => out.push_str("no click-tone pairs found\n"),
        Some(s) => {
            let _ = writeln!(
                out,
                "latency ms: min {:.2}  p10 {:.2}  p50 {:.2}  p90 {:.2}  p99 {:.2}  max {:.2}  \
                 mean {:.2}  stddev {:.2}",
                s.min, s.p10, s.p50, s.p90, s.p99, s.max, s.mean, s.stddev
            );
        }
    }
    out
}

/// CSV of the pairs: `pair,click_s,tone_s,latency_ms`.
pub fn write_csv(mut out: impl std::io::Write, analysis: &Analysis) -> std::io::Result<()> {
    writeln!(out, "pair,click_s,tone_s,latency_ms")?;
    for (i, p) in analysis.pairs.iter().enumerate() {
        writeln!(
            out,
            "{i},{:.6},{:.6},{:.3}",
            p.click_s,
            p.tone_s,
            p.latency_ms()
        )?;
    }
    out.flush()
}

/// Loads the recording, analyses it and prints the report.
pub fn run(options: &Options) -> Result<(), String> {
    let path = &options.file;
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str());
    let sample = Sample::load(&bytes, ext, SAMPLE_RATE, 1)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let analysis = analyze(
        sample.data(),
        SAMPLE_RATE,
        options.tone_hz,
        options.window_ms,
    );
    print!("{}", report(&path.display().to_string(), &analysis));
    if let Some(dump) = &options.dump {
        std::fs::File::create(dump)
            .and_then(|file| write_csv(std::io::BufWriter::new(file), &analysis))
            .map_err(|e| format!("--dump {}: {e}", dump.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// Deterministic xorshift noise in [-1, 1).
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
        }
    }

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    fn add_click(signal: &mut [f32], at_s: f64, noise: &mut Noise, amplitude: f32) {
        let start = (at_s * f64::from(RATE)).round() as usize;
        for k in 0..(RATE as usize / 100) {
            let decay = (-(k as f32) / 48.0).exp();
            signal[start + k] += amplitude * decay * noise.next();
        }
    }

    fn add_tone(signal: &mut [f32], at_s: f64, hz: f64, gain: f32) {
        let burst = tone_burst(RATE, 1, hz, 30.0);
        // Fractional start: shift the phase and envelope by resynthesizing is overkill;
        // the delay is rounded to the sample.
        let start = (at_s * f64::from(RATE)).round() as usize;
        for (k, &v) in burst.data().iter().enumerate() {
            signal[start + k] += gain * v;
        }
    }

    /// Returns the signal and the true (click, tone) times.
    fn recording(
        delays_ms: &[f64],
        noise_level: f32,
        release: bool,
    ) -> (Vec<f32>, Vec<(f64, f64)>) {
        let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
        let seconds = 0.5 + 0.6 * delays_ms.len() as f64 + 0.5;
        let mut signal: Vec<f32> = (0..(seconds * f64::from(RATE)) as usize)
            .map(|_| noise_level * noise.next())
            .collect();
        let mut truth = Vec::new();
        for (i, &delay) in delays_ms.iter().enumerate() {
            let press = 0.5 + 0.6 * i as f64;
            // Tone starts on a sample boundary after the delay.
            let tone = ((press + delay / 1000.0) * f64::from(RATE)).round() / f64::from(RATE);
            add_click(&mut signal, press, &mut noise, 0.4 + 0.1 * (i % 3) as f32);
            if release {
                add_click(&mut signal, press + 0.080, &mut noise, 0.3);
            }
            add_tone(&mut signal, tone, 1000.0, 0.5 + 0.5 * (i % 2) as f32);
            truth.push((press, tone));
        }
        (signal, truth)
    }

    const DELAYS: [f64; 6] = [12.0, 15.5, 20.25, 13.1, 17.9, 14.4];

    #[test]
    fn measures_known_latencies() {
        let (signal, truth) = recording(&DELAYS, 0.003, true);
        let a = analyze(&signal, RATE, 1000.0, 100.0);
        assert_eq!(a.clicks.len(), 12, "{:?}", a.clicks);
        assert_eq!(a.tones.len(), 6, "{:?}", a.tones);
        assert_eq!(a.pairs.len(), 6);
        for (pair, (click, tone)) in a.pairs.iter().zip(&truth) {
            let want = (tone - click) * 1000.0;
            let err = pair.latency_ms() - want;
            eprintln!("latency {:.3} ms, truth {want:.3} ms", pair.latency_ms());
            assert!(err.abs() < 0.25, "{} vs {want}", pair.latency_ms());
        }
    }

    #[test]
    fn works_without_release_clicks_and_narrow_window() {
        let (signal, truth) = recording(&DELAYS[..3], 0.002, false);
        let a = analyze(&signal, RATE, 1000.0, 50.0);
        assert_eq!((a.clicks.len(), a.pairs.len()), (3, 3));
        for (pair, (click, tone)) in a.pairs.iter().zip(&truth) {
            assert!((pair.latency_ms() - (tone - click) * 1000.0).abs() < 0.25);
        }
    }

    #[test]
    fn tone_without_click_is_unpaired() {
        let (mut signal, _) = recording(&DELAYS[..2], 0.003, true);
        add_tone(&mut signal, 2.0, 1000.0, 0.5);
        let a = analyze(&signal, RATE, 1000.0, 100.0);
        assert_eq!(a.tones.len(), 3);
        assert_eq!(a.pairs.len(), 2);
        assert_eq!(a.clicks.len(), 4);
    }

    #[test]
    fn click_without_tone_is_unpaired() {
        let (mut signal, _) = recording(&DELAYS[..2], 0.003, false);
        let mut noise = Noise(7);
        add_click(&mut signal, 2.0, &mut noise, 0.4);
        let a = analyze(&signal, RATE, 1000.0, 100.0);
        assert_eq!((a.clicks.len(), a.tones.len(), a.pairs.len()), (3, 2, 2));
    }

    #[test]
    fn quiet_recording_has_no_pairs() {
        let mut noise = Noise(3);
        let signal: Vec<f32> = (0..RATE as usize * 2)
            .map(|_| 0.001 * noise.next())
            .collect();
        let a = analyze(&signal, RATE, 1000.0, 100.0);
        assert!(a.pairs.is_empty() && a.tones.is_empty());
        assert!(analyze(&[], RATE, 1000.0, 100.0).pairs.is_empty());
        assert!(
            analyze(&vec![0.0; 48_000], RATE, 1000.0, 100.0)
                .clicks
                .is_empty()
        );
        assert!(report("quiet.wav", &a).contains("no click-tone pairs"));
    }

    #[test]
    fn pairing_rules() {
        // The release click (0.08) is after the tone; the second click is too far back.
        let pairs = pair(&[0.0, 0.08, 1.0], &[0.015, 1.3], 0.1);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].click_s, 0.0);
        // The earliest click in the window wins.
        let pairs = pair(&[0.9, 0.95], &[1.0], 0.2);
        assert_eq!(pairs[0].click_s, 0.9);
        // A click closer than 0.5 ms before the tone does not count.
        assert!(pair(&[0.9996], &[1.0], 0.1).is_empty());
    }

    #[test]
    fn stats_nearest_rank() {
        let values: Vec<f64> = (1..=100).rev().map(f64::from).collect();
        let s = Stats::of(&values).unwrap();
        assert_eq!(
            (s.min, s.p10, s.p50, s.p90, s.p99, s.max),
            (1.0, 10.0, 50.0, 90.0, 99.0, 100.0)
        );
        assert!((s.mean - 50.5).abs() < 1e-9);
        assert!(Stats::of(&[]).is_none());
    }

    #[test]
    fn csv_has_header_and_rows() {
        let a = Analysis {
            pairs: vec![Pair {
                click_s: 1.0,
                tone_s: 1.015,
            }],
            ..Analysis::default()
        };
        let mut out = Vec::new();
        write_csv(&mut out, &a).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "pair,click_s,tone_s,latency_ms\n0,1.000000,1.015000,15.000\n"
        );
    }

    #[test]
    fn options() {
        let o = parse(&["rec.wav"]).unwrap();
        assert_eq!(o.file, PathBuf::from("rec.wav"));
        assert_eq!(
            (o.tone_hz, o.window_ms, o.dump.clone()),
            (1000.0, 100.0, None)
        );
        let o = parse(&[
            "--tone-hz",
            "2000",
            "a.wav",
            "--window-ms",
            "50",
            "--dump",
            "o.csv",
        ])
        .unwrap();
        assert_eq!((o.tone_hz, o.window_ms), (2000.0, 50.0));
        assert_eq!(o.dump, Some(PathBuf::from("o.csv")));
        assert!(parse(&[]).is_err());
        assert!(parse(&["a.wav", "b.wav"]).is_err());
        assert!(parse(&["a.wav", "--tone-hz", "30000"]).is_err());
        assert!(parse(&["a.wav", "--window-ms", "0"]).is_err());
        assert!(parse(&["a.wav", "--dump"]).is_err());
        assert!(parse(&["a.wav", "--what"]).is_err());
    }
}
