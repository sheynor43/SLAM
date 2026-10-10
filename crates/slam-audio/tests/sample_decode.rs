//! Decoding hitsound files in every accepted format.
//!
//! The files in `tests/data` hold the same signal: 10 ms of silence, then a 1 kHz sine
//! at amplitude 0.5 until 100 ms, 44.1 kHz. Generated with
//! `ffmpeg -f lavfi -i "aevalsrc=if(gte(t\,0.01)\,0.5*sin(2*PI*1000*t)\,0):s=44100:d=0.1"`
//! and the codec options `-c:a pcm_s16le | pcm_u8 | adpcm_ms`,
//! `-af "pan=stereo|c0=c0|c1=c0" -c:a libvorbis -q:a 4`, `-c:a libmp3lame -b:a 128k`.

use slam_audio::{Sample, SampleError};

const RATE: u32 = 44_100;
const FRAMES: usize = 4_410;
const ONSET: usize = 441;

fn load(name: &str) -> Sample {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap();
    let extension = name.rsplit('.').next();
    Sample::decode(&bytes, extension).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// First frame of channel 0 louder than `threshold`.
fn onset(sample: &Sample, threshold: f32) -> usize {
    let channels = usize::from(sample.channels());
    sample
        .data()
        .chunks_exact(channels)
        .position(|f| f[0].abs() > threshold)
        .unwrap()
}

/// Largest absolute value of channel 0 between `from` and `to` frames.
fn peak(sample: &Sample, from: usize, to: usize) -> f32 {
    let channels = usize::from(sample.channels());
    sample.data()[from * channels..to * channels]
        .chunks_exact(channels)
        .map(|f| f[0].abs())
        .fold(0.0, f32::max)
}

#[test]
fn pcm_wav_decodes_exactly() {
    let s = load("tone-s16-mono.wav");
    assert_eq!(
        (s.sample_rate(), s.channels(), s.frames()),
        (RATE, 1, FRAMES)
    );
    for (n, &got) in s.data().iter().enumerate() {
        let t = n as f64 / f64::from(RATE);
        let want = if n >= ONSET {
            0.5 * (std::f64::consts::TAU * 1_000.0 * t).sin()
        } else {
            0.0
        };
        assert!(
            (f64::from(got) - want).abs() < 1e-3,
            "frame {n}: {got} vs {want}"
        );
    }
}

#[test]
fn every_format_keeps_rate_onset_and_level() {
    // Lossy codecs smear the onset by a few frames (ADPCM adapts its step size); MP3 must not add its encoder delay
    // (over 1000 frames), which the decoder trims (gapless). ADPCM pads the last block
    // with silence (2036 frames per block here); the `fact` length is not applied.
    for (name, channels, onset_tolerance, extra_frames) in [
        ("tone-u8-mono.wav", 1, 1, 0),
        ("tone-adpcm-mono.wav", 1, 6, 2_036),
        ("tone-vorbis-stereo.ogg", 2, 20, 64),
        ("tone-mp3-mono.mp3", 1, 20, 64),
    ] {
        let s = load(name);
        assert_eq!((s.sample_rate(), s.channels()), (RATE, channels), "{name}");
        assert!(
            s.frames() + 64 >= FRAMES && s.frames() <= FRAMES + extra_frames,
            "{name}: {} frames",
            s.frames()
        );
        let at = onset(&s, 0.05);
        assert!(
            at.abs_diff(ONSET) <= onset_tolerance,
            "{name}: onset at {at}"
        );
        let level = peak(&s, ONSET + 100, FRAMES - 200);
        assert!((level - 0.5).abs() < 0.05, "{name}: peak {level}");
        if channels == 2 {
            let (l, r): (Vec<f32>, Vec<f32>) = s
                .data()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[a, b]| (a, b))
                .unzip();
            let diff = l
                .iter()
                .zip(&r)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(diff < 0.05, "{name}: channels differ by {diff}");
        }
    }
}

#[test]
fn load_converts_to_the_device_format() {
    let s = Sample::load(
        &std::fs::read(format!(
            "{}/tests/data/tone-vorbis-stereo.ogg",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
        Some("ogg"),
        48_000,
        1,
    )
    .unwrap();
    assert_eq!((s.sample_rate(), s.channels()), (48_000, 1));
    let onset_48k = ONSET * 48_000 / 44_100;
    assert!(onset(&s, 0.05).abs_diff(onset_48k) <= 24);
}

#[test]
fn garbage_and_empty_input_fail() {
    assert!(matches!(
        Sample::decode(&[0x55; 4096], Some("wav")),
        Err(SampleError::Decode(_))
    ));
    assert!(Sample::decode(&[], None).is_err());
    // A valid header cut off right after it: no frames, no format.
    let wav = std::fs::read(format!(
        "{}/tests/data/tone-s16-mono.wav",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    assert!(Sample::decode(&wav[..44], Some("wav")).is_err());
}
