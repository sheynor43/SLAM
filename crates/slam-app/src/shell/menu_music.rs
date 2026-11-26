//! Music outside gameplay (ADR-0090): the selected beatmap's preview on the song select, a
//! random beatmap on the main menu, with lazer's transitions. Two decks take turns, so
//! that the old track fades out while the new one fades in. Runs on the update thread:
//! allocates only when it starts another track.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use slam_audio::{Easing, MusicPlayer, TrackAdjustment, TrackSource};

/// Old track out (lazer's `MusicController.changeTrack`).
const FADE_OUT_MS: f64 = 150.0;
/// New track in, after [`DELAY_BEFORE_FADE_MS`].
const FADE_IN_MS: f64 = 220.0;
/// lazer's `MusicController.DELAY_BEFORE_FADE`; also how far before the preview point the
/// song select starts.
const DELAY_BEFORE_FADE_MS: f64 = 30.0;
/// A preview starting from silence rises this long (lazer's song select duck restore).
const RISE_MS: f64 = 800.0;
/// The first menu track starts this far before its preview point and rises this long
/// (lazer's `IntroScreen.StartTrack`).
const MENU_LEAD_MS: f64 = 2_600.0;
/// After a track failed to open, the menu waits this long before trying another.
const RETRY_NS: u64 = 1_000_000_000;
/// Tracks the menu remembers for the player's Previous button (ADR-0178).
pub const HISTORY: usize = 16;
/// Previous restarts the track when it is further than this into it, else goes back
/// (lazer's `MusicController.PreviousTrack`).
pub const RESTART_AFTER_MS: f64 = 5_000.0;

/// What the screens want to hear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Want<'a> {
    /// Nothing: gameplay owns the music, or the setting is off.
    Silence,
    /// Whatever plays; a stopped track plays on.
    Keep,
    /// The song select's beatmap: its preview, looping.
    Preview { path: &'a Path, preview_ms: i32 },
    /// The main menu: what plays, else a random beatmap.
    Menu,
}

/// A track the menu picked.
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    pub path: PathBuf,
    pub preview_ms: i32,
    /// Set and difficulty in the library: whose background the menu shows (ADR-0119).
    pub at: (usize, usize),
}

/// One music player, as the menu uses it.
pub trait Deck {
    /// Starts loading `path`, paused; its generation, `None` when the queue is full.
    fn load(&mut self, path: &Path) -> Option<u32>;
    fn seek(&mut self, frame: u64);
    fn play(&mut self);
    fn pause(&mut self);
    fn fade(&mut self, volume: f32, delay_ms: f64, duration_ms: f64, easing: Easing);
    /// Whether the track played to its end since the last load or seek.
    fn ended(&self) -> bool;
    /// Track frames per second: the mixer's rate (ADR-0102).
    fn sample_rate(&self) -> u32;
}

impl Deck for MusicPlayer {
    fn load(&mut self, path: &Path) -> Option<u32> {
        MusicPlayer::load(self, TrackSource::File(path.to_path_buf())).ok()
    }

    // A full queue drops these: the next change of track or screen sends again.
    fn seek(&mut self, frame: u64) {
        let _ = MusicPlayer::seek(self, frame);
    }

    fn play(&mut self) {
        let _ = MusicPlayer::play(self);
    }

    fn pause(&mut self) {
        let _ = MusicPlayer::pause(self);
    }

    fn fade(&mut self, volume: f32, delay_ms: f64, duration_ms: f64, easing: Easing) {
        let _ = self.adjust(&[TrackAdjustment::Fade {
            volume,
            delay_ms,
            duration_ms,
            easing,
        }]);
    }

    fn sample_rate(&self) -> u32 {
        MusicPlayer::sample_rate(self)
    }

    fn ended(&self) -> bool {
        self.position()
            .read()
            .is_some_and(|m| m.ended && m.generation == self.generation())
    }
}

/// How a loaded track comes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    /// From silence: rises over [`RISE_MS`].
    Rise,
    /// Over a track going out: waits, then fades in.
    Cross,
    /// The menu's first track: from before its preview point, rising slowly.
    MenuIntro,
    /// The menu's next track: from the start, faded in.
    MenuNext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Opening; `held` when it was stopped before it opened: it then waits paused at its
    /// starting point.
    Loading {
        entry: Entry,
        held: bool,
    },
    Playing,
    /// Fading out until `until_ns`, then paused.
    Stopping {
        until_ns: u64,
    },
    Paused,
}

#[derive(Clone, Debug)]
struct Slot {
    track: Option<PathBuf>,
    phase: Phase,
    /// The generation of the track's load; seeks count up from it.
    generation: u32,
    /// Where the preview starts again when the track loops, track frames.
    restart: u64,
    looping: bool,
    preview_ms: i32,
    /// The track's length in frames, once it opened.
    length: Option<u64>,
}

impl Slot {
    fn new() -> Slot {
        Slot {
            track: None,
            phase: Phase::Idle,
            generation: 0,
            restart: 0,
            looping: false,
            preview_ms: -1,
            length: None,
        }
    }

    fn holds(&self, path: &Path) -> bool {
        self.track.as_deref() == Some(path) && self.phase != Phase::Idle
    }

    /// Whether `generation` is this track's: its load or a later seek.
    fn owns(&self, generation: u32) -> bool {
        self.phase != Phase::Idle && generation.wrapping_sub(self.generation) < 1 << 31
    }
}

/// The state of both decks; the decks themselves are passed in each call.
pub struct MenuMusic {
    slots: [Slot; 2],
    /// The deck heard, or about to be.
    active: usize,
    /// A track that failed to open, not tried again while it stays wanted.
    failed: Option<PathBuf>,
    /// No random track before this.
    retry_at_ns: u64,
    /// No track at all before this: the deck's queue was full.
    load_at_ns: u64,
    /// The menu's track ended: the next comes in from its start.
    menu_next: bool,
    /// The player paused the music: nothing resumes it but the player (ADR-0178).
    user_paused: bool,
    /// The tracks the menu played before the current one, oldest first.
    history: VecDeque<Picked>,
    /// The track the menu is playing, as it picked it; `None` for a song select preview.
    current: Option<Picked>,
}

/// What the player's Previous button did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Previous {
    /// No track.
    Nothing,
    /// The track went back to its start.
    Restarted,
    /// The track before this one started: the beatmap at this set and difficulty.
    Started((usize, usize)),
}

impl Default for MenuMusic {
    fn default() -> Self {
        MenuMusic {
            slots: [Slot::new(), Slot::new()],
            active: 0,
            failed: None,
            retry_at_ns: 0,
            load_at_ns: 0,
            menu_next: false,
            user_paused: false,
            history: VecDeque::with_capacity(HISTORY),
            current: None,
        }
    }
}

impl MenuMusic {
    /// The deck heard, or about to be.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Follows `want` at `now_ns`; `pick` gives a random beatmap for the menu.
    pub fn tick<D: Deck>(
        &mut self,
        now_ns: u64,
        want: Want<'_>,
        decks: &mut [D; 2],
        pick: impl FnOnce() -> Option<Picked>,
    ) {
        self.finish_stops(now_ns, decks);
        self.loop_or_next(now_ns, want, decks);
        let active = self.active;
        match want {
            Want::Silence => self.stop(active, now_ns, decks),
            Want::Keep => {
                self.resume_or_hold(decks);
            }
            Want::Preview { path, preview_ms } => {
                if self.slots[active].holds(path) {
                    // A menu track the song select selected loops from now on, as lazer's
                    // song select makes the track loop.
                    self.slots[active].looping = true;
                    self.resume_or_hold(decks);
                } else if self.failed.as_deref() != Some(path) && now_ns >= self.load_at_ns {
                    self.start(now_ns, path, preview_ms, true, decks);
                }
            }
            Want::Menu => {
                // The menu moves on at the end of a preview it took over.
                self.slots[active].looping = false;
                if !self.resume_or_hold(decks)
                    && self.slots[active].phase == Phase::Idle
                    && now_ns >= self.retry_at_ns.max(self.load_at_ns)
                {
                    match pick() {
                        Some(picked) => {
                            self.start_menu(now_ns, picked, decks);
                        }
                        None => self.retry_at_ns = now_ns.saturating_add(RETRY_NS),
                    }
                }
            }
        }
    }

    /// The track of `generation` on deck `deck` opened, `length` frames long.
    pub fn loaded<D: Deck>(
        &mut self,
        deck: usize,
        generation: u32,
        length: Option<u64>,
        decks: &mut [D; 2],
    ) {
        let slot = &mut self.slots[deck];
        let Phase::Loading { entry, held } = slot.phase else {
            return;
        };
        if generation != slot.generation {
            return;
        }
        slot.length = length;
        let d = &mut decks[deck];
        let frames_per_ms = f64::from(d.sample_rate()) / 1_000.0;
        let point = preview_point(slot.preview_ms, length, frames_per_ms);
        slot.restart = clamp_frame(point - DELAY_BEFORE_FADE_MS * frames_per_ms, length);
        match entry {
            Entry::MenuIntro => d.seek(clamp_frame(point - MENU_LEAD_MS * frames_per_ms, length)),
            Entry::Rise | Entry::Cross => d.seek(slot.restart),
            Entry::MenuNext => {}
        }
        if held {
            // Rises from where it waits when wanted again.
            slot.phase = Phase::Paused;
            return;
        }
        d.play();
        match entry {
            Entry::Rise => d.fade(1.0, 0.0, RISE_MS, Easing::OutQuint),
            Entry::Cross | Entry::MenuNext => {
                d.fade(1.0, DELAY_BEFORE_FADE_MS, FADE_IN_MS, Easing::None);
            }
            Entry::MenuIntro => d.fade(1.0, 0.0, MENU_LEAD_MS, Easing::InCubic),
        }
        slot.phase = Phase::Playing;
    }

    /// The track of `generation` on deck `deck` failed, opening or later.
    pub fn failed<D: Deck>(
        &mut self,
        now_ns: u64,
        deck: usize,
        generation: u32,
        decks: &mut [D; 2],
    ) {
        let slot = &mut self.slots[deck];
        if slot.owns(generation) {
            decks[deck].pause();
            slot.phase = Phase::Idle;
            self.failed = slot.track.take();
            self.retry_at_ns = now_ns.saturating_add(RETRY_NS);
        }
    }

    /// Starts `path` on the other deck; the active one fades out.
    fn start<D: Deck>(
        &mut self,
        now_ns: u64,
        path: &Path,
        preview_ms: i32,
        looping: bool,
        decks: &mut [D; 2],
    ) -> bool {
        let old = self.active;
        let audible = matches!(
            self.slots[old].phase,
            Phase::Playing | Phase::Loading { held: false, .. }
        );
        let entry = match (looping, audible, self.menu_next) {
            (true, true, _) => Entry::Cross,
            (true, false, _) => Entry::Rise,
            (false, true, _) | (false, false, true) => Entry::MenuNext,
            (false, false, false) => Entry::MenuIntro,
        };
        self.stop(old, now_ns, decks);
        let new = 1 - old;
        let d = &mut decks[new];
        d.fade(0.0, 0.0, 0.0, Easing::None);
        let Some(generation) = d.load(path) else {
            self.load_at_ns = now_ns.saturating_add(RETRY_NS);
            return false;
        };
        self.menu_next = false;
        self.failed = None;
        self.active = new;
        let slot = &mut self.slots[new];
        // Allocates: another track.
        slot.track = Some(path.to_path_buf());
        slot.generation = generation;
        slot.restart = 0;
        slot.preview_ms = preview_ms;
        slot.looping = looping;
        slot.length = None;
        // A track chosen while the player has paused waits paused at its start.
        slot.phase = Phase::Loading {
            entry,
            held: self.user_paused,
        };
        if looping {
            // A preview is not a menu track: it has no place in the history.
            self.current = None;
        }
        true
    }

    /// Starts the menu's `picked` track; the one it replaces goes into the history.
    fn start_menu<D: Deck>(&mut self, now_ns: u64, picked: Picked, decks: &mut [D; 2]) -> bool {
        if !self.start(now_ns, &picked.path, picked.preview_ms, false, decks) {
            return false;
        }
        if let Some(old) = self.current.replace(picked) {
            if self.history.len() == HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(old);
        }
        true
    }

    /// Plays the active deck's stopped track on unless the player paused it; whether it has
    /// a track.
    fn resume_or_hold<D: Deck>(&mut self, decks: &mut [D; 2]) -> bool {
        if self.user_paused {
            return self.slots[self.active].phase != Phase::Idle;
        }
        self.resume(decks)
    }

    /// Whether the player paused the music.
    pub fn is_paused(&self) -> bool {
        self.user_paused
    }

    /// Whether a track is loaded or loading on the active deck.
    pub fn has_track(&self) -> bool {
        self.slots[self.active].phase != Phase::Idle
    }

    /// How many tracks Previous can go back through.
    #[cfg(test)]
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// The active track's length in milliseconds, once it opened.
    pub fn length_ms<D: Deck>(&self, decks: &[D; 2]) -> Option<f64> {
        let frames = self.slots[self.active].length?;
        let rate = f64::from(decks[self.active].sample_rate());
        (rate > 0.0).then(|| frames as f64 / rate * 1_000.0)
    }

    /// The player's Play/Pause: pauses the music (fading out) or plays it on. Without a
    /// track it only remembers the choice, so a track that starts is held.
    pub fn toggle_pause<D: Deck>(&mut self, now_ns: u64, decks: &mut [D; 2]) {
        if self.user_paused {
            self.user_paused = false;
            self.resume(decks);
        } else {
            self.user_paused = true;
            let active = self.active;
            self.stop(active, now_ns, decks);
        }
    }

    /// The player's seek bar: the active track to `ms`.
    pub fn seek_ms<D: Deck>(&mut self, ms: f64, decks: &mut [D; 2]) {
        let slot = &self.slots[self.active];
        if !matches!(
            slot.phase,
            Phase::Playing | Phase::Paused | Phase::Stopping { .. }
        ) {
            return;
        }
        let d = &mut decks[self.active];
        let frames_per_ms = f64::from(d.sample_rate()) / 1_000.0;
        let last = slot.length.map(|l| l.saturating_sub(1));
        d.seek(clamp_frame(ms.max(0.0) * frames_per_ms, last));
    }

    /// The player's Next on the main menu: another random track, from its start. Returns
    /// the beatmap it started.
    pub fn next<D: Deck>(
        &mut self,
        now_ns: u64,
        decks: &mut [D; 2],
        pick: impl FnOnce() -> Option<Picked>,
    ) -> Option<(usize, usize)> {
        let picked = pick()?;
        let at = picked.at;
        self.menu_next = true;
        self.start_menu(now_ns, picked, decks).then_some(at)
    }

    /// The player's Previous on the main menu: past [`RESTART_AFTER_MS`] into the track, or
    /// without history, the track starts over; else the track before it comes back.
    pub fn previous<D: Deck>(
        &mut self,
        now_ns: u64,
        position_ms: Option<f64>,
        decks: &mut [D; 2],
    ) -> Previous {
        if !self.has_track() {
            return Previous::Nothing;
        }
        let far = position_ms.is_some_and(|ms| ms > RESTART_AFTER_MS);
        if far || self.history.is_empty() {
            decks[self.active].seek(0);
            return Previous::Restarted;
        }
        let Some(picked) = self.history.pop_back() else {
            return Previous::Nothing;
        };
        let at = picked.at;
        self.menu_next = true;
        if self.start(now_ns, &picked.path, picked.preview_ms, false, decks) {
            self.current = Some(picked);
            Previous::Started(at)
        } else {
            self.history.push_back(picked);
            Previous::Nothing
        }
    }

    /// Fades deck `deck` out; it pauses once faded.
    fn stop<D: Deck>(&mut self, deck: usize, now_ns: u64, decks: &mut [D; 2]) {
        let slot = &mut self.slots[deck];
        slot.phase = match slot.phase {
            Phase::Playing => {
                decks[deck].fade(0.0, 0.0, FADE_OUT_MS, Easing::Out);
                Phase::Stopping {
                    until_ns: now_ns.saturating_add((FADE_OUT_MS * 1e6) as u64),
                }
            }
            // A track not heard yet waits paused.
            Phase::Loading { entry, .. } => Phase::Loading { entry, held: true },
            other => other,
        };
    }

    /// Plays the active deck's stopped track on, rising; whether it has one.
    fn resume<D: Deck>(&mut self, decks: &mut [D; 2]) -> bool {
        let slot = &mut self.slots[self.active];
        match slot.phase {
            Phase::Stopping { .. } | Phase::Paused => {
                let d = &mut decks[self.active];
                d.play();
                d.fade(1.0, 0.0, RISE_MS, Easing::OutQuint);
                slot.phase = Phase::Playing;
                true
            }
            Phase::Loading { entry, .. } => {
                slot.phase = Phase::Loading { entry, held: false };
                true
            }
            Phase::Playing => true,
            Phase::Idle => false,
        }
    }

    /// Pauses the decks that faded out.
    fn finish_stops<D: Deck>(&mut self, now_ns: u64, decks: &mut [D; 2]) {
        for (slot, deck) in self.slots.iter_mut().zip(decks.iter_mut()) {
            if let Phase::Stopping { until_ns } = slot.phase
                && now_ns >= until_ns
            {
                deck.pause();
                slot.phase = Phase::Paused;
            }
        }
    }

    /// At the end of the active track: a preview starts again at its point, the menu
    /// moves to the next random track.
    fn loop_or_next<D: Deck>(&mut self, now_ns: u64, want: Want<'_>, decks: &mut [D; 2]) {
        let active = self.active;
        let slot = &mut self.slots[active];
        if slot.phase != Phase::Playing || !decks[active].ended() {
            return;
        }
        if slot.looping {
            decks[active].seek(slot.restart);
            decks[active].play();
        } else if want == Want::Menu || want == Want::Keep {
            // The next `Menu` tick picks another.
            decks[active].pause();
            slot.phase = Phase::Idle;
            slot.track = None;
            self.retry_at_ns = now_ns;
            self.menu_next = true;
        }
    }
}

/// lazer's preview point, track frames: `PreviewTime` when within the track, else 40 % of
/// its length.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/WorkingBeatmap.cs (PrepareTrackForPreview)
fn preview_point(preview_ms: i32, length: Option<u64>, frames_per_ms: f64) -> f64 {
    let length = length.map_or(f64::INFINITY, |l| l as f64);
    let point = f64::from(preview_ms) * frames_per_ms;
    if point < 0.0 || point > length {
        if length.is_finite() {
            // lazer multiplies by the `float` 0.4.
            f64::from(0.4f32) * length
        } else {
            0.0
        }
    } else {
        point
    }
}

fn clamp_frame(frame: f64, length: Option<u64>) -> u64 {
    let max = length.map_or(f64::MAX, |l| l as f64);
    frame.clamp(0.0, max) as u64
}

#[cfg(test)]
mod tests;
