//! Hitsound selection on a beatmap folder, a user skin and a default skin built in temporary
//! directories.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use slam_osu::judgement::{HitResult, Judgement, JudgementLog, ObjectRef};
use slam_osu::objects::{NestedKind, OsuHitObjectKind};
use slam_osu::replay::{Actions, Rules, StepInput};
use slam_osu::rules::OsuRules;
use slam_osu::{Beatmap, ModSet, Vec2};
use slam_play::hitsounds::{Hitsound, Hitsounds, SampleCache, SampleSkins, Sound};
use slam_skin::{Skin, SkinChain};
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

/// Timing points: normal bank, custom index 0, volume 60 from 0 ms; soft bank, custom index
/// 2, volume 3 from 2000 ms.
const MAP: &str = "osu file format v14

[General]
AudioFilename: audio.mp3
LAYERED_LINE

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:5
ApproachRate:9
SliderMultiplier:1.4
SliderTickRate:1

[TimingPoints]
0,300,4,1,0,60,1,0
2000,-100,4,2,2,3,0,0

[HitObjects]
100,100,500,1,0,0:0:2:0:
100,100,700,1,2,0:0:0:0:
100,100,900,1,0,0:0:1:70:
100,100,1100,1,0,0:0:0:0:kick.wav
100,100,1300,1,2,0:0:0:0:
100,100,1500,2,2,L|300:100,1,200,2|2,0:0|0:0,0:0:0:0:
100,100,2100,2,2,L|300:100,2,200,0|8|2,1:0|2:0|3:0,0:0:0:0:
256,192,3000,12,0,4500,0:0:0:0:
";

const CUSTOM_INDEX_2: u32 = 0;
const LAYERED_WHISTLE: u32 = 1;
const CUSTOM_INDEX_1: u32 = 2;
const FILE_SAMPLE: u32 = 3;
const LAYERED_WHISTLE_AGAIN: u32 = 4;
const WHISTLE_SLIDER: u32 = 5;
const SLIDER: u32 = 6;
const SPINNER: u32 = 7;

/// A short 16-bit mono WAV.
fn wav() -> Vec<u8> {
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

/// A temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "slam-hitsounds-{}-{}-{name}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn samples(self, names: &[&str]) -> TempDir {
        for name in names {
            fs::write(self.0.join(name), wav()).unwrap();
        }
        self
    }

    fn file(self, name: &str, bytes: &[u8]) -> TempDir {
        fs::write(self.0.join(name), bytes).unwrap();
        self
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    osu: String,
    beatmap_dir: TempDir,
    user_dir: TempDir,
    default_dir: TempDir,
    beatmap_hitsounds: bool,
}

/// What a resolved fixture plays, with sounds named `<dir>/<file>`.
struct Resolved {
    map: Beatmap,
    sounds: Hitsounds,
    cache: SampleCache,
    files: Vec<(String, PathBuf)>,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            osu: MAP.replace("LAYERED_LINE", ""),
            beatmap_dir: TempDir::new("beatmap").samples(&[
                "normal-hitnormal2.wav",
                "normal-hitnormal.wav",
                "kick.wav",
                "drum-hitwhistle2.wav",
            ]),
            user_dir: TempDir::new("user").samples(&[
                "normal-hitnormal.wav",
                "normal-hitnormal2.wav",
                "normal-hitwhistle.wav",
                "soft-hitnormal.wav",
                "soft-hitclap.wav",
                "drum-hitnormal.wav",
                "soft-sliderslide.wav",
                "soft-spinnerspin.wav",
            ]),
            default_dir: TempDir::new("default").samples(&[
                "normal-hitnormal.wav",
                "drum-hitwhistle.wav",
                "sliderwhistle.wav",
                "soft-slidertick.wav",
                "spinnerbonus.wav",
            ]),
            beatmap_hitsounds: true,
        }
    }

    fn resolve(&self) -> Resolved {
        let map = Beatmap::decode(self.osu.as_bytes()).unwrap();
        let beatmap_skin = Skin::load_beatmap(&self.beatmap_dir.0, self.osu.as_bytes()).unwrap();
        let chain = SkinChain::new(
            Some(Skin::load(&self.user_dir.0).unwrap()),
            Skin::load(&self.default_dir.0).unwrap(),
        );
        let skins = SampleSkins {
            beatmap: self.beatmap_hitsounds.then_some(&beatmap_skin),
            chain: &chain,
        };
        let mut cache = SampleCache::new(48_000, 2);
        let sounds = Hitsounds::new(&map, skins, &mut cache);
        let mut files = Vec::new();
        for (label, dir) in [
            ("beatmap", &self.beatmap_dir),
            ("user", &self.user_dir),
            ("default", &self.default_dir),
        ] {
            for entry in fs::read_dir(&dir.0).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                files.push((format!("{label}/{name}"), path));
            }
        }
        Resolved {
            map,
            sounds,
            cache,
            files,
        }
    }
}

impl Resolved {
    fn name_of(&mut self, path: &Path) -> Option<Arc<slam_audio::Sample>> {
        self.cache.get(path)
    }

    /// The files and gains of `sounds`.
    fn names(&mut self, sounds: &[Hitsound]) -> Vec<(String, f32)> {
        sounds
            .iter()
            .map(|s| {
                let files = self.files.clone();
                let name = files
                    .iter()
                    .find(|(_, path)| {
                        self.name_of(path)
                            .is_some_and(|decoded| Arc::ptr_eq(&decoded, &s.sample))
                    })
                    .map_or_else(|| "?".to_owned(), |(name, _)| name.clone());
                (name, s.gain)
            })
            .collect()
    }

    fn play(&mut self, index: u32, sound: Sound) -> Vec<(String, f32)> {
        let sounds = self.sounds.get(index, sound).to_vec();
        self.names(&sounds)
    }

    fn files_of(&mut self, index: u32, sound: Sound) -> Vec<String> {
        self.play(index, sound)
            .into_iter()
            .map(|(n, _)| n)
            .collect()
    }
}

#[test]
fn custom_sample_banks_and_file_samples_come_from_the_beatmap() {
    let mut r = Fixture::new().resolve();
    assert_eq!(
        r.play(CUSTOM_INDEX_2, Sound::Hit),
        [("beatmap/normal-hitnormal2.wav".to_owned(), 0.6)]
    );
    // Custom index 0: the beatmap's `normal-hitnormal.wav` is not used.
    assert_eq!(
        r.files_of(LAYERED_WHISTLE, Sound::Hit),
        ["user/normal-hitnormal.wav", "user/normal-hitwhistle.wav"]
    );
    // Custom index 1: beatmap samples without a suffix.
    assert_eq!(
        r.play(CUSTOM_INDEX_1, Sound::Hit),
        [("beatmap/normal-hitnormal.wav".to_owned(), 0.7)]
    );
    assert_eq!(
        r.play(FILE_SAMPLE, Sound::Hit),
        [("beatmap/kick.wav".to_owned(), 0.6)]
    );
}

#[test]
fn skins_never_use_custom_sample_bank_suffixes() {
    let mut fixture = Fixture::new();
    fixture.beatmap_hitsounds = false;
    let mut r = fixture.resolve();
    // The user skin has `normal-hitnormal2.wav` but must play `normal-hitnormal.wav`.
    assert_eq!(
        r.files_of(CUSTOM_INDEX_2, Sound::Hit),
        ["user/normal-hitnormal.wav"]
    );
    // Without beatmap hitsounds, a file sample falls back to the skin's `hitnormal`.
    assert_eq!(
        r.files_of(FILE_SAMPLE, Sound::Hit),
        ["user/normal-hitnormal.wav"]
    );
}

#[test]
fn an_undecodable_file_is_silent_like_lazer() {
    let mut fixture = Fixture::new();
    fixture.beatmap_dir = fixture
        .beatmap_dir
        .file("normal-hitnormal2.wav", b"not a wav");
    let r = fixture.resolve();
    // Lazer's lookup stops at the first file found; BASS then plays nothing.
    assert!(r.sounds.get(CUSTOM_INDEX_2, Sound::Hit).is_empty());
}

#[test]
fn a_skin_without_layered_hit_sounds_silences_them_without_fallback() {
    let mut fixture = Fixture::new();
    fixture.user_dir = fixture
        .user_dir
        .file("skin.ini", b"[General]\nLayeredHitSounds: 0\n");
    let mut r = fixture.resolve();
    // The default skin has `normal-hitnormal.wav`, but the user skin answers first.
    assert_eq!(
        r.files_of(LAYERED_WHISTLE, Sound::Hit),
        ["user/normal-hitwhistle.wav"]
    );
    // Sliding samples made from a layered normal are layered too.
    assert_eq!(
        r.files_of(WHISTLE_SLIDER, Sound::Sliding),
        ["default/sliderwhistle.wav"]
    );
    // A normal sample that is not layered still plays.
    assert_eq!(
        r.files_of(CUSTOM_INDEX_1, Sound::Hit),
        ["beatmap/normal-hitnormal.wav"]
    );
}

#[test]
fn the_beatmap_configuration_can_silence_layered_samples() {
    let mut fixture = Fixture::new();
    fixture.osu = MAP.replace("LAYERED_LINE", "LayeredHitSounds: 0");
    let mut r = fixture.resolve();
    // The beatmap skin is asked first even for custom index 0, and its rule applies.
    assert_eq!(
        r.files_of(LAYERED_WHISTLE, Sound::Hit),
        ["user/normal-hitwhistle.wav"]
    );

    fixture.beatmap_hitsounds = false;
    let mut r = fixture.resolve();
    assert_eq!(
        r.files_of(LAYERED_WHISTLE, Sound::Hit),
        ["user/normal-hitnormal.wav", "user/normal-hitwhistle.wav"]
    );
}

#[test]
fn slider_nodes_ticks_and_sliding_samples() {
    let mut r = Fixture::new().resolve();
    let OsuHitObjectKind::Slider(slider) = &r.map.hit_objects[SLIDER as usize].kind else {
        panic!("not a slider");
    };
    let nested: Vec<(u32, NestedKind)> = slider
        .nested
        .iter()
        .enumerate()
        .map(|(i, n)| (i as u32, n.kind))
        .collect();
    let index_of = |pred: fn(&NestedKind) -> bool| {
        nested
            .iter()
            .find(|(_, k)| pred(k))
            .map(|(i, _)| *i)
            .unwrap()
    };
    let head = index_of(|k| matches!(k, NestedKind::Head));
    let tick = index_of(|k| matches!(k, NestedKind::Tick { .. }));
    let repeat = index_of(|k| matches!(k, NestedKind::Repeat { .. }));
    let tail = index_of(|k| matches!(k, NestedKind::Tail { .. }));

    // Custom index 2 and volume 3 come from the sample point; the volume plays at the
    // minimum of 5.
    assert_eq!(
        r.play(SLIDER, Sound::Nested(head)),
        [("beatmap/normal-hitnormal2.wav".to_owned(), 0.05)]
    );
    assert_eq!(
        r.files_of(SLIDER, Sound::Nested(repeat)),
        ["user/soft-hitnormal.wav", "user/soft-hitclap.wav"]
    );
    assert_eq!(
        r.files_of(SLIDER, Sound::Nested(tick)),
        ["default/soft-slidertick.wav"]
    );
    assert!(r.sounds.get(SLIDER, Sound::Nested(tail)).is_empty());
    // The slider's own hit plays the tail node.
    assert_eq!(
        r.files_of(SLIDER, Sound::Hit),
        ["user/drum-hitnormal.wav", "beatmap/drum-hitwhistle2.wav"]
    );
    assert_eq!(
        r.files_of(SLIDER, Sound::Sliding),
        ["user/soft-sliderslide.wav", "default/sliderwhistle.wav"]
    );
}

#[test]
fn spinner_sounds() {
    let mut r = Fixture::new().resolve();
    let OsuHitObjectKind::Spinner(spinner) = &r.map.hit_objects[SPINNER as usize].kind else {
        panic!("not a spinner");
    };
    let tick_count = spinner.tick_count() as u32;

    assert_eq!(r.files_of(SPINNER, Sound::Hit), ["user/soft-hitnormal.wav"]);
    assert_eq!(
        r.files_of(SPINNER, Sound::Spinning),
        ["user/soft-spinnerspin.wav"]
    );
    // `spinnerbonus-max` falls back to the `spinnerbonus` names.
    assert_eq!(
        r.files_of(SPINNER, Sound::SpinnerBonusMax),
        ["default/spinnerbonus.wav"]
    );

    // Without input the rules judge every tick when the spinner ends; each plays the bonus
    // sample exactly when it is a bonus tick.
    let mut rules = OsuRules::new(&r.map, &ModSet::default());
    let mut log = JudgementLog::with_capacity(rules.max_judgements());
    for time in 0..6_000 {
        let step = StepInput {
            time: f64::from(time),
            position: Vec2::new(0.0, 0.0),
            held: Actions::NONE,
            events: &[],
        };
        rules.update(&step, &mut log);
    }
    let ticks: Vec<_> = log
        .events()
        .iter()
        .filter(|j| j.object.index == SPINNER && j.object.nested.is_some())
        .copied()
        .collect();
    assert_eq!(ticks.len(), tick_count as usize);
    assert!(ticks.iter().any(|j| j.max_result == HitResult::LargeBonus));
    for judgement in &ticks {
        let sounds = r.files_of(SPINNER, Sound::Nested(judgement.object.nested.unwrap()));
        if judgement.max_result == HitResult::LargeBonus {
            assert_eq!(sounds, ["default/spinnerbonus.wav"]);
        } else {
            assert!(sounds.is_empty());
        }
    }
    assert!(r.sounds.get(SPINNER, Sound::Nested(tick_count)).is_empty());
}

#[test]
fn objects_share_one_sample_per_file() {
    let r = Fixture::new().resolve();
    let a = &r.sounds.get(LAYERED_WHISTLE, Sound::Hit)[0];
    let b = &r.sounds.get(LAYERED_WHISTLE_AGAIN, Sound::Hit)[0];
    assert!(Arc::ptr_eq(&a.sample, &b.sample));
    // The spinner's bonus and bonus-max sounds are one file.
    let bonus = &r.sounds.get(SPINNER, Sound::SpinnerBonusMax)[0];
    let spinner = &r.map.hit_objects[SPINNER as usize];
    let OsuHitObjectKind::Spinner(s) = &spinner.kind else {
        unreachable!()
    };
    let tick = &r
        .sounds
        .get(SPINNER, Sound::Nested(s.spins_required_for_bonus() as u32))[0];
    assert!(Arc::ptr_eq(&bonus.sample, &tick.sample));
}

#[test]
fn out_of_range_lookups_are_empty() {
    let r = Fixture::new().resolve();
    assert!(r.sounds.get(99, Sound::Hit).is_empty());
    assert!(r.sounds.get(SLIDER, Sound::Nested(999)).is_empty());
    assert!(r.sounds.get(CUSTOM_INDEX_2, Sound::Nested(0)).is_empty());
    assert!(r.sounds.get(CUSTOM_INDEX_2, Sound::Sliding).is_empty());
}

#[test]
fn lookups_do_not_allocate() {
    let r = Fixture::new().resolve();
    let sounds = &r.sounds;
    assert_no_alloc(|| {
        let mut total = 0;
        for index in 0..r.map.hit_objects.len() as u32 {
            for sound in [
                Sound::Hit,
                Sound::Nested(0),
                Sound::Nested(3),
                Sound::Sliding,
                Sound::Spinning,
                Sound::SpinnerBonusMax,
            ] {
                total += sounds.get(index, sound).len();
            }
        }
        std::hint::black_box(total);
    });
}

#[test]
fn only_hits_play() {
    let r = Fixture::new().resolve();
    let judgement = |result| Judgement {
        object: ObjectRef {
            index: CUSTOM_INDEX_2,
            nested: None,
        },
        time: 500.0,
        result,
        max_result: HitResult::Great,
    };
    assert_eq!(r.sounds.for_judgement(&judgement(HitResult::Meh)).len(), 1);
    assert!(
        r.sounds
            .for_judgement(&judgement(HitResult::Miss))
            .is_empty()
    );
}

#[test]
fn a_beatmap_folder_can_be_dropped_from_the_cache() {
    let fixture = Fixture::new();
    let mut r = fixture.resolve();
    let before = r.cache.len();
    r.cache.remove_dir(&fixture.beatmap_dir.0);
    // Kick, normal-hitnormal, normal-hitnormal2 and drum-hitwhistle2 of the beatmap.
    assert_eq!(r.cache.len(), before - 4);
}
