//! Sounds of interacting with the menus (ADR-0120): the update thread notes what happened
//! ([`UiSoundQueue`]); the play host plays SLAM's own synthesized sounds or the skin's
//! ([`UiSoundPlayer`]). The skin's are read and decoded on a thread of their own, which
//! also frees replaced ones.

use std::path::Path;
use std::sync::Arc;

use slam_audio::{MixerHandle, Sample, VoiceGroup, VoiceParams};

use crate::menu_sounds::{self, MenuSound};
use crate::settings::MenuSounds;

const SOUNDS: usize = MenuSound::ALL.len();

/// The same sound plays again no sooner than this: hovering along a list, holding a key.
const REPEAT_NS: [u64; SOUNDS] = {
    let mut gaps = [0; SOUNDS];
    let mut i = 0;
    while i < SOUNDS {
        gaps[i] = match MenuSound::ALL[i] {
            MenuSound::Hover => 50_000_000,
            MenuSound::SliderStep => 30_000_000,
            _ => 0,
        };
        i += 1;
    }
    gaps
};

fn index(sound: MenuSound) -> usize {
    MenuSound::ALL
        .iter()
        .position(|s| *s == sound)
        .unwrap_or_default()
}

/// What happened in the menus since the host last looked. Never allocates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiSoundQueue {
    pending: [bool; SOUNDS],
}

impl UiSoundQueue {
    pub fn push(&mut self, sound: MenuSound) {
        self.pending[index(sound)] = true;
    }

    /// The sounds noted, in [`MenuSound::ALL`] order; clears them.
    pub fn take(&mut self) -> [bool; SOUNDS] {
        std::mem::take(&mut self.pending)
    }
}

/// The skin's sounds, as read off the update thread.
type SkinSounds = [Option<Arc<Sample>>; SOUNDS];

/// How long skin sounds replaced stay alive before they go to be freed: voices playing them
/// end, and the mixer hands its references back, before the last reference leaves.
const RETIRE_NS: u64 = 3_000_000_000;

/// Replaced skin sounds waiting for [`RETIRE_NS`].
const RETIRING: usize = 4;

/// The reading thread's ends.
struct Reader {
    requests: rtrb::Producer<Arc<Path>>,
    read: rtrb::Consumer<(Arc<Path>, SkinSounds)>,
    /// Sounds to free on the thread.
    old: rtrb::Producer<SkinSounds>,
}

/// Plays the noted sounds on the mixer. Allocates only when the skin or the setting
/// changes, and never frees on the update thread.
pub struct UiSoundPlayer {
    rate: u32,
    /// Kept for good, like the intro's sound.
    own: [Option<Arc<Sample>>; SOUNDS],
    skin: SkinSounds,
    /// The skin folder whose sounds are wanted, and whether they were asked for.
    skin_for: Option<Arc<Path>>,
    asked: bool,
    reader: Option<Reader>,
    retiring: [Option<(u64, SkinSounds)>; RETIRING],
    last_ns: [Option<u64>; SOUNDS],
}

impl UiSoundPlayer {
    /// SLAM's own sounds at the output's `rate`; the mixer plays them at any other.
    pub fn new(rate: u32) -> UiSoundPlayer {
        UiSoundPlayer {
            rate,
            own: own_sounds(rate),
            skin: Default::default(),
            skin_for: None,
            asked: false,
            reader: None,
            retiring: Default::default(),
            last_ns: [None; SOUNDS],
        }
    }

    /// Follows the skin folder `skin` while `mode` takes the skin's sounds: the reading
    /// thread reads them when it changes. Takes what it read; hands replaced sounds to it
    /// to free once they are old enough.
    pub fn follow_skin(&mut self, now_ns: u64, mode: MenuSounds, skin: Option<&Arc<Path>>) {
        self.retire(now_ns);
        if let Some(reader) = &mut self.reader {
            while let Ok((path, sounds)) = reader.read.pop() {
                if self
                    .skin_for
                    .as_ref()
                    .is_some_and(|wanted| Arc::ptr_eq(wanted, &path))
                {
                    let old = std::mem::replace(&mut self.skin, sounds);
                    Self::put_away(&mut self.retiring, reader, now_ns, old);
                } else if let Err(rtrb::PushError::Full(sounds)) = reader.old.push(sounds) {
                    // Never played: only the reader's thread stalled.
                    std::mem::forget(sounds);
                }
            }
        }
        if mode != MenuSounds::Skin {
            return;
        }
        if skin.map(|s| &**s) != self.skin_for.as_deref() {
            self.skin_for = skin.cloned();
            self.asked = false;
            let old = std::mem::take(&mut self.skin);
            if let Some(reader) = &mut self.reader {
                Self::put_away(&mut self.retiring, reader, now_ns, old);
            } else {
                std::mem::forget(old);
            }
        }
        if self.asked {
            return;
        }
        let Some(dir) = self.skin_for.clone() else {
            self.asked = true;
            return;
        };
        if self.reader.is_none() {
            self.reader = spawn_reader(self.rate);
        }
        if let Some(reader) = &mut self.reader {
            // A full ring asks again on the next tick.
            self.asked = reader.requests.push(dir).is_ok();
        }
    }

    /// Keeps `old` until it is old enough to free; with every slot taken it is let go.
    fn put_away(
        retiring: &mut [Option<(u64, SkinSounds)>; RETIRING],
        reader: &mut Reader,
        now_ns: u64,
        old: SkinSounds,
    ) {
        if old.iter().all(Option::is_none) {
            return;
        }
        match retiring.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some((now_ns + RETIRE_NS, old)),
            None => {
                if let Err(rtrb::PushError::Full(old)) = reader.old.push(old) {
                    std::mem::forget(old);
                }
            }
        }
    }

    /// Hands sounds old enough to the reading thread to free.
    fn retire(&mut self, now_ns: u64) {
        let Some(reader) = &mut self.reader else {
            return;
        };
        for slot in &mut self.retiring {
            if slot.as_ref().is_some_and(|(at, _)| now_ns >= *at)
                && let Some((at, sounds)) = slot.take()
                && let Err(rtrb::PushError::Full(sounds)) = reader.old.push(sounds)
            {
                *slot = Some((at, sounds));
            }
        }
    }

    /// Plays the `noted` sounds at `now_ns` as `mode` says.
    pub fn play(
        &mut self,
        now_ns: u64,
        noted: [bool; SOUNDS],
        mode: MenuSounds,
        mixer: &mut MixerHandle,
    ) {
        if mode == MenuSounds::Off || !noted.contains(&true) {
            return;
        }
        for (i, _) in noted.iter().enumerate().filter(|(_, noted)| **noted) {
            if self.last_ns[i].is_some_and(|last| now_ns.saturating_sub(last) < REPEAT_NS[i]) {
                continue;
            }
            let skin = match mode {
                MenuSounds::Skin => self.skin[i].as_ref(),
                _ => None,
            };
            // A sound the skin lacks is SLAM's own (ADR-0120).
            let Some(sample) = skin.or(self.own[i].as_ref()) else {
                continue;
            };
            self.last_ns[i] = Some(now_ns);
            let params = VoiceParams {
                group: VoiceGroup::Effects,
                ..VoiceParams::default()
            };
            let _ = mixer.play_voice(sample, params);
        }
    }
}

/// Starts the thread that reads skins' sounds at `rate` and frees the old ones.
fn spawn_reader(rate: u32) -> Option<Reader> {
    let (requests, mut requests_rx) = rtrb::RingBuffer::<Arc<Path>>::new(2);
    let (mut read_tx, read) = rtrb::RingBuffer::new(2);
    let (old, mut old_rx) = rtrb::RingBuffer::<SkinSounds>::new(RETIRING * 2);
    let spawned = std::thread::Builder::new()
        .name("slam-menu-sounds".to_owned())
        .spawn(move || {
            loop {
                while old_rx.pop().is_ok() {}
                let mut newest = None;
                while let Ok(dir) = requests_rx.pop() {
                    newest = Some(dir);
                }
                if let Some(dir) = newest {
                    let sounds = skin_sounds(&dir, rate);
                    let _ = read_tx.push((dir, sounds));
                }
                if requests_rx.is_abandoned() && old_rx.is_abandoned() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
    match spawned {
        Ok(_) => Some(Reader {
            requests,
            read,
            old,
        }),
        Err(e) => {
            tracing::warn!("menu sounds: cannot start the thread: {e}");
            None
        }
    }
}

fn own_sounds(rate: u32) -> [Option<Arc<Sample>>; SOUNDS] {
    MenuSound::ALL.map(|sound| {
        Sample::from_interleaved(menu_sounds::render(sound, rate), rate, 2)
            .inspect_err(|e| tracing::warn!("menu sound {sound:?}: {e}"))
            .ok()
            .map(Arc::new)
    })
}

/// The skin's sound for each of [`MenuSound::ALL`], by its names in lookup order; `None`
/// where the skin has none or it does not decode.
fn skin_sounds(dir: &Path, rate: u32) -> SkinSounds {
    let Ok(skin) = slam_skin::Skin::load(dir)
        .inspect_err(|e| tracing::warn!("menu sounds: skin {}: {e}", dir.display()))
    else {
        return Default::default();
    };
    MenuSound::ALL.map(|sound| {
        sound.skin_names().iter().find_map(|name| {
            let path = skin.sample(name)?;
            let bytes = std::fs::read(&path).ok()?;
            let extension = path.extension().and_then(|e| e.to_str());
            Sample::load(&bytes, extension, rate, 2)
                .inspect_err(|e| tracing::warn!("menu sound {}: {e}", path.display()))
                .ok()
                .map(Arc::new)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(frames: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        let data = frames * 4;
        for chunk in [&b"RIFF"[..], &(36 + data).to_le_bytes(), b"WAVEfmt "] {
            bytes.extend_from_slice(chunk);
        }
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&44_100u32.to_le_bytes());
        bytes.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0u8, data as usize));
        bytes
    }

    #[test]
    fn the_queue_notes_each_sound_once_until_taken() {
        let mut queue = UiSoundQueue::default();
        queue.push(MenuSound::Hover);
        queue.push(MenuSound::Hover);
        queue.push(MenuSound::Back);
        let taken = queue.take();
        assert_eq!(taken.iter().filter(|n| **n).count(), 2);
        assert!(taken[index(MenuSound::Hover)] && taken[index(MenuSound::Back)]);
        assert_eq!(queue.take(), [false; SOUNDS]);
    }

    #[test]
    fn own_sounds_render_at_the_rate() {
        let player = UiSoundPlayer::new(48_000);
        assert!(player.own.iter().all(Option::is_some));
    }

    #[test]
    fn the_skins_sounds_are_found_by_their_names() {
        let dir = std::env::temp_dir().join(format!("slam-ui-sounds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("menuback.wav"), wav(441)).unwrap();
        std::fs::write(dir.join("Click-Short.wav"), wav(441)).unwrap();
        let sounds = skin_sounds(&dir, 48_000);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(sounds[index(MenuSound::Back)].is_some());
        assert!(sounds[index(MenuSound::Hover)].is_some());
        assert!(sounds[index(MenuSound::SliderStep)].is_some());
        assert!(sounds[index(MenuSound::Enter)].is_none());
    }

    #[test]
    fn the_reader_reads_the_skin_and_retires_the_old_sounds_late() {
        let dir = std::env::temp_dir().join(format!("slam-ui-reader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("menuback.wav"), wav(441)).unwrap();
        let skin: Arc<Path> = Arc::from(dir.as_path());
        let mut player = UiSoundPlayer::new(48_000);
        // Off the skin mode nothing is read.
        player.follow_skin(0, MenuSounds::Slam, Some(&skin));
        assert!(player.reader.is_none());
        player.follow_skin(0, MenuSounds::Skin, Some(&skin));
        let mut waited = 0;
        while player.skin[index(MenuSound::Back)].is_none() && waited < 200 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            player.follow_skin(0, MenuSounds::Skin, Some(&skin));
            waited += 1;
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(player.skin[index(MenuSound::Back)].is_some());
        // Another skin: the old sounds wait before they go.
        player.follow_skin(1, MenuSounds::Skin, None);
        assert!(player.skin.iter().all(Option::is_none));
        assert!(player.retiring.iter().any(Option::is_some));
        player.follow_skin(1 + RETIRE_NS, MenuSounds::Skin, None);
        assert!(player.retiring.iter().all(Option::is_none));
    }

    #[test]
    fn repeats_are_spaced_for_hover_and_slider_steps_only() {
        assert_eq!(REPEAT_NS[index(MenuSound::Hover)], 50_000_000);
        assert_eq!(REPEAT_NS[index(MenuSound::SliderStep)], 30_000_000);
        assert_eq!(REPEAT_NS[index(MenuSound::Click)], 0);
    }
}
