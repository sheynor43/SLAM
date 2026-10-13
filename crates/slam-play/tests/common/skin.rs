//! A default skin of short WAV samples in a temporary directory.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use slam_audio::Sample;
use slam_osu::Beatmap;
use slam_play::{Hitsounds, SampleCache, SampleSkins};
use slam_skin::{Skin, SkinChain};

/// A short 16-bit mono WAV.
pub fn wav() -> Vec<u8> {
    let pcm: Vec<u8> = (0..64i16).flat_map(|i| (i * 100).to_le_bytes()).collect();
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&44_100u32.to_le_bytes());
    wav.extend_from_slice(&88_200u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(&pcm);
    wav
}

/// A temporary directory of samples, removed on drop.
pub struct SampleDir(pub PathBuf);

impl SampleDir {
    pub fn new(names: &[&str]) -> SampleDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "slam-play-skin-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        for name in names {
            fs::write(dir.join(name), wav()).unwrap();
        }
        SampleDir(dir)
    }
}

impl Drop for SampleDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The hitsounds of `map` from a default skin with the sample files `names`, and the decoded
/// sample of each file by name.
pub fn hitsounds(
    map: &Beatmap,
    names: &[&'static str],
) -> (Hitsounds, Vec<(&'static str, Arc<Sample>)>) {
    let dir = SampleDir::new(names);
    let chain = SkinChain::new(None, Skin::load(&dir.0).unwrap());
    let skins = SampleSkins {
        beatmap: None,
        chain: &chain,
    };
    let mut cache = SampleCache::new(48_000, 2);
    let sounds = Hitsounds::new(map, skins, &mut cache);
    let samples = names
        .iter()
        .map(|&name| (name, cache.get(&dir.0.join(name)).unwrap()))
        .collect();
    (sounds, samples)
}
