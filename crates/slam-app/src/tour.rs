//! `SLAM_TOUR=<folder>`: the real game walks itself through its screens and saves a PNG of
//! each, for the README's pictures. It plays as usual — the player's own library, skin,
//! settings and music — while a script presses keys at fixed times since the first tick;
//! the draw thread reads the frame back when asked and a thread of its own writes it. A
//! developer tool, off unless the variable is set: it allocates and writes files, which
//! the game otherwise never does on its hot paths. OpenGL only (Direct3D 11 cannot read
//! its frames back yet).

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;

use slam_input::{InputEvent, InputKind};

/// What the script does at a moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Presses and releases these keys (scancodes), in order: held ones first.
    Keys(&'static [u16]),
    /// Selects this mod alone (an acronym), for a play the tour can watch.
    Mod(&'static str),
    /// Saves the frame as `<name>.png`.
    Capture(&'static str),
    /// Closes the game.
    Exit,
}

const CTRL: u16 = 224;
const KEY_O: u16 = 18;
const KEY_P: u16 = 19;
const KEY_F1: u16 = 58;
const KEY_ENTER: u16 = 40;
const KEY_ESCAPE: u16 = 41;
const KEY_SPACE: u16 = 44;

/// The walk, in ms since the first tick: the intro's explosion, the settings, the main menu
/// alone and open, the song select and its mods, then a play watched by Autoplay.
pub const SCRIPT: &[(u64, Step)] = &[
    (2_150, Step::Capture("intro")),
    (4_500, Step::Keys(&[CTRL, KEY_O])),
    (6_000, Step::Capture("settings")),
    (6_200, Step::Keys(&[KEY_ESCAPE])),
    (9_000, Step::Capture("menu")),
    (9_200, Step::Keys(&[KEY_P])),
    (10_600, Step::Capture("menu-open")),
    (10_800, Step::Keys(&[KEY_P])),
    (15_000, Step::Capture("song-select")),
    (15_200, Step::Keys(&[KEY_F1])),
    (16_500, Step::Capture("mods")),
    (16_700, Step::Keys(&[KEY_F1])),
    (17_200, Step::Mod("AT")),
    (17_500, Step::Keys(&[KEY_ENTER])),
    (23_000, Step::Keys(&[KEY_SPACE])),
    (33_000, Step::Capture("gameplay")),
    (35_000, Step::Exit),
];

/// The update thread's side: where in the script it is.
#[derive(Debug)]
pub struct Tour {
    start_ns: Option<u64>,
    next: usize,
    /// Counts the captures asked for; the draw thread saves each new one.
    seq: u32,
    events: Vec<InputEvent>,
}

/// What the tour asks of this tick.
#[derive(Debug, Default)]
pub struct TourTick<'a> {
    /// Key events to add to the tick's own.
    pub events: &'a [InputEvent],
    /// A mod to select alone.
    pub select_mod: Option<&'static str>,
    /// Close the game.
    pub exit: bool,
}

impl Tour {
    /// The tour, when `SLAM_TOUR` names a folder (made if missing), and the draw thread's
    /// camera writing into it.
    pub fn from_env() -> Option<(Tour, Camera)> {
        let dir = PathBuf::from(std::env::var_os("SLAM_TOUR")?);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::error!("tour: {}: {e}", dir.display());
            return None;
        }
        tracing::info!("tour: saving screens to {}", dir.display());
        Some((
            Tour {
                start_ns: None,
                next: 0,
                seq: 0,
                events: Vec::new(),
            },
            Camera::new(dir),
        ))
    }

    /// The capture asked for last: its count and name, for the snapshot.
    pub fn capture(&self) -> (u32, &'static str) {
        let name = SCRIPT[..self.next]
            .iter()
            .rev()
            .find_map(|(_, step)| match step {
                Step::Capture(name) => Some(*name),
                _ => None,
            })
            .unwrap_or("");
        (self.seq, name)
    }

    /// The steps due at `now_ns`.
    pub fn tick(&mut self, now_ns: u64) -> TourTick<'_> {
        let start = *self.start_ns.get_or_insert(now_ns);
        let elapsed_ms = now_ns.saturating_sub(start) / 1_000_000;
        self.events.clear();
        let mut select_mod = None;
        let mut exit = false;
        while let Some(&(at, step)) = SCRIPT.get(self.next) {
            if at > elapsed_ms {
                break;
            }
            self.next += 1;
            match step {
                Step::Keys(keys) => {
                    let key = |scancode, down| InputEvent {
                        time_ns: now_ns,
                        kind: InputKind::Key { scancode, down },
                    };
                    self.events.extend(keys.iter().map(|&k| key(k, true)));
                    self.events
                        .extend(keys.iter().rev().map(|&k| key(k, false)));
                }
                Step::Mod(acronym) => select_mod = Some(acronym),
                Step::Capture(name) => {
                    self.seq += 1;
                    tracing::info!("tour: {name}");
                }
                Step::Exit => exit = true,
            }
        }
        TourTick {
            events: &self.events,
            select_mod,
            exit,
        }
    }
}

/// A frame to save.
struct Shot {
    name: &'static str,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// The draw thread's side: notices new captures and hands their frames to the writer.
#[derive(Debug)]
pub struct Camera {
    seen: u32,
    sender: Option<mpsc::Sender<Shot>>,
    writer: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Shot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shot").field("name", &self.name).finish()
    }
}

impl Camera {
    fn new(dir: PathBuf) -> Camera {
        let (sender, receiver) = mpsc::channel::<Shot>();
        let writer = std::thread::Builder::new()
            .name("slam-tour".into())
            .spawn(move || {
                for shot in receiver {
                    let frame = crate::screenshot::Frame {
                        width: shot.width,
                        height: shot.height,
                        pixels: shot.pixels,
                    };
                    let path = dir.join(format!("{}.png", shot.name));
                    match frame.write_png(&path) {
                        Ok(()) => tracing::info!("tour: wrote {}", path.display()),
                        Err(e) => tracing::error!("tour: {e}"),
                    }
                }
            })
            .ok();
        Camera {
            seen: 0,
            sender: Some(sender),
            writer,
        }
    }

    /// The name to save the frame as, when `capture` (from the snapshot) is new.
    pub fn due(&mut self, capture: (u32, &'static str)) -> Option<&'static str> {
        (capture.0 != self.seen).then(|| {
            self.seen = capture.0;
            capture.1
        })
    }

    /// Hands a `width` × `height` RGBA frame to the writer.
    pub fn save(&self, name: &'static str, width: u32, height: u32, pixels: Vec<u8>) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(Shot {
                name,
                width,
                height,
                pixels,
            });
        }
    }
}

impl Drop for Camera {
    /// Waits for the writer to save what it was given.
    fn drop(&mut self) {
        self.sender.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn tour() -> Tour {
        Tour {
            start_ns: None,
            next: 0,
            seq: 0,
            events: Vec::new(),
        }
    }

    #[test]
    fn the_script_runs_in_time_order_and_ends() {
        assert!(SCRIPT.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(SCRIPT.last().map(|s| s.1), Some(Step::Exit));
    }

    #[test]
    fn steps_come_when_due_with_their_keys_pressed_and_released() {
        let mut tour = tour();
        let start = 1_000 * MS;
        assert!(tour.tick(start).events.is_empty());
        assert_eq!(tour.capture(), (0, ""));
        let tick = tour.tick(start + 2_200 * MS);
        assert!(tick.events.is_empty());
        assert_eq!(tour.capture(), (1, "intro"));
        let keys: Vec<_> = tour
            .tick(start + 4_600 * MS)
            .events
            .iter()
            .map(|e| e.kind)
            .collect();
        let key = |scancode, down| InputKind::Key { scancode, down };
        assert_eq!(
            keys,
            [
                key(CTRL, true),
                key(KEY_O, true),
                key(KEY_O, false),
                key(CTRL, false)
            ]
        );
        assert!(tour.tick(start + 40_000 * MS).exit);
    }

    #[test]
    fn the_camera_saves_each_capture_once() {
        let mut camera = Camera {
            seen: 0,
            sender: None,
            writer: None,
        };
        assert_eq!(camera.due((0, "")), None);
        assert_eq!(camera.due((1, "menu")), Some("menu"));
        assert_eq!(camera.due((1, "menu")), None);
    }
}
