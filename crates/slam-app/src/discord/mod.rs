//! Discord rich presence (ADR-0170): what the player does, shown on their Discord profile.
//!
//! The update tick calls [`Discord::follow`] every tick; it compares a small key and builds
//! strings and sends a [`Presence`] to the `slam-discord` thread only when the key changes.
//! The thread does all the I/O.

mod ipc;
mod worker;

use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use slam_ui::i18n::{FluentArgs, Localizer};

use worker::Msg;

/// The application ID of "SLAM" on discord.com/developers. While it is `"0"`, the project
/// owner has not registered the application yet and no connection is ever made; the owner
/// replaces it with the real ID.
pub const DISCORD_APP_ID: &str = "0";

/// What the player is doing, as far as the status tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    Menu,
    SongSelect,
    /// Playing the play of this id.
    Playing(u64),
    /// The play of this id is paused or failed.
    Paused(u64),
    Results,
}

/// The beatmap of a play, as the status names it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Play {
    /// "artist - title [difficulty]".
    pub title: String,
    /// The mods' acronyms, separated by commas; empty without mods.
    pub mods: String,
}

/// One status, as Discord shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence {
    /// The first line.
    pub details: String,
    /// The second line.
    pub state: Option<String>,
    /// When the play started, in seconds since the Unix epoch.
    pub start: Option<u64>,
}

/// The activity object of `SET_ACTIVITY`.
pub fn activity_json(presence: &Presence) -> Value {
    let mut activity = json!({ "details": presence.details });
    if let Some(state) = &presence.state {
        activity["state"] = json!(state);
    }
    if let Some(start) = presence.start {
        activity["timestamps"] = json!({ "start": start });
    }
    activity
}

/// The first frame's payload.
pub fn handshake_payload(app_id: &str) -> Value {
    json!({ "v": 1, "client_id": app_id })
}

/// The payload of a `SET_ACTIVITY` command; `None` clears the status.
pub fn set_activity_payload(pid: u32, presence: Option<&Presence>, nonce: u64) -> Value {
    json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": pid, "activity": presence.map(activity_json) },
        "nonce": nonce.to_string(),
    })
}

/// The status for `scene`; `play` names the beatmap of a play, `start` is its start time.
pub fn presence(scene: Scene, play: &Play, start: Option<u64>, localizer: &Localizer) -> Presence {
    let text = |key: &str| localizer.format(key, None).into_owned();
    let with_mods = |plain: &str, with: &str| {
        if play.mods.is_empty() {
            text(plain)
        } else {
            let mut args = FluentArgs::new();
            args.set("mods", play.mods.as_str());
            localizer.format(with, Some(&args)).into_owned()
        }
    };
    match scene {
        Scene::Menu => line(text("discord-menu")),
        Scene::SongSelect => line(text("discord-song-select")),
        Scene::Results => line(text("discord-results")),
        Scene::Playing(_) => Presence {
            details: play.title.clone(),
            state: Some(with_mods("discord-playing", "discord-playing-mods")),
            start,
        },
        Scene::Paused(_) => Presence {
            details: play.title.clone(),
            state: Some(with_mods("discord-paused", "discord-paused-mods")),
            start: None,
        },
    }
}

fn line(details: String) -> Presence {
    Presence {
        details,
        state: None,
        start: None,
    }
}

/// The update side: remembers what was last sent and sends a status when it changes.
pub struct Discord {
    /// The thread's inbox; `None` without an application ID or when the thread would not
    /// start.
    tx: Option<Sender<Msg>>,
    /// What the shown status stands for; `None` is "cleared".
    sent: Option<Scene>,
    /// The play whose start time is `started.1`.
    started: Option<(u64, u64)>,
}

impl Discord {
    /// Starts the thread, unless the application ID is not filled in yet.
    pub fn spawn() -> Discord {
        let tx = if DISCORD_APP_ID == "0" {
            None
        } else {
            worker::spawn(DISCORD_APP_ID)
                .inspect_err(|e| tracing::warn!("discord: cannot start the thread: {e}"))
                .ok()
        };
        Discord::with_sender(tx)
    }

    /// A status that never sends anything.
    pub fn disabled() -> Discord {
        Discord::with_sender(None)
    }

    fn with_sender(tx: Option<Sender<Msg>>) -> Discord {
        Discord {
            tx,
            sent: None,
            started: None,
        }
    }

    /// Shows `scene` while `enabled`, else clears the status. Called every tick: does
    /// nothing, allocation included, until `scene` or `enabled` changes. `play` names the
    /// beatmap and is called only when a change needs it.
    pub fn follow(
        &mut self,
        enabled: bool,
        scene: Scene,
        play: impl FnOnce() -> Play,
        localizer: &Localizer,
    ) {
        if self.tx.is_none() {
            return;
        }
        let wanted = enabled.then_some(scene);
        if wanted == self.sent {
            return;
        }
        self.sent = wanted;
        let message = match wanted {
            None => Msg::Set(None),
            Some(scene) => {
                let start = match scene {
                    Scene::Playing(id) | Scene::Paused(id) => Some(self.start_of(id)),
                    _ => None,
                };
                let play = match scene {
                    Scene::Playing(_) | Scene::Paused(_) => play(),
                    _ => Play::default(),
                };
                Msg::Set(Some(presence(scene, &play, start, localizer)))
            }
        };
        // The thread ending means the status is gone; nothing to do about it.
        if let Some(tx) = &self.tx {
            let _ = tx.send(message);
        }
    }

    /// When play `id` started: now, the first time it is asked.
    fn start_of(&mut self, id: u64) -> u64 {
        match self.started {
            Some((started_id, at)) if started_id == id => at,
            _ => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                self.started = Some((id, now));
                now
            }
        }
    }
}

impl Drop for Discord {
    fn drop(&mut self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Stop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn en() -> Localizer {
        let localizer = Localizer::builtin().unwrap();
        localizer.set_language("en").unwrap();
        localizer
    }

    fn play() -> Play {
        Play {
            title: "Artist - Title [Insane]".to_owned(),
            mods: "HD,DT".to_owned(),
        }
    }

    #[test]
    fn the_menu_and_the_song_select_have_one_line() {
        let l = en();
        let menu = presence(Scene::Menu, &Play::default(), None, &l);
        assert_eq!(
            activity_json(&menu),
            json!({ "details": "In the main menu" })
        );
        let select = presence(Scene::SongSelect, &Play::default(), None, &l);
        assert_eq!(
            activity_json(&select),
            json!({ "details": "Choosing a beatmap" })
        );
        let results = presence(Scene::Results, &Play::default(), None, &l);
        assert_eq!(
            activity_json(&results),
            json!({ "details": "Looking at the results" })
        );
    }

    #[test]
    fn a_play_shows_the_beatmap_the_mods_and_the_start() {
        let p = presence(Scene::Playing(1), &play(), Some(1_700_000_000), &en());
        assert_eq!(
            activity_json(&p),
            json!({
                "details": "Artist - Title [Insane]",
                "state": "Playing with HD,DT",
                "timestamps": { "start": 1_700_000_000u64 },
            })
        );
        let plain = Play {
            mods: String::new(),
            ..play()
        };
        let p = presence(Scene::Playing(1), &plain, Some(5), &en());
        assert_eq!(p.state.as_deref(), Some("Playing"));
    }

    #[test]
    fn a_pause_shows_the_beatmap_without_a_clock() {
        let p = presence(Scene::Paused(1), &play(), Some(5), &en());
        assert_eq!(
            activity_json(&p),
            json!({
                "details": "Artist - Title [Insane]",
                "state": "Paused with HD,DT",
            })
        );
    }

    #[test]
    fn the_russian_texts_exist() {
        let l = Localizer::builtin().unwrap();
        l.set_language("ru").unwrap();
        for scene in [Scene::Menu, Scene::SongSelect, Scene::Results] {
            let p = presence(scene, &Play::default(), None, &l);
            assert!(!p.details.starts_with("discord-"), "{scene:?}");
        }
        let p = presence(Scene::Playing(1), &play(), Some(1), &l);
        assert!(!p.state.unwrap().starts_with("discord-"));
    }

    #[test]
    fn the_commands_carry_the_pid_and_a_nonce() {
        let p = line("x".to_owned());
        assert_eq!(
            set_activity_payload(42, Some(&p), 3),
            json!({
                "cmd": "SET_ACTIVITY",
                "args": { "pid": 42, "activity": { "details": "x" } },
                "nonce": "3",
            })
        );
        assert!(set_activity_payload(42, None, 4)["args"]["activity"].is_null());
        assert_eq!(
            handshake_payload("99"),
            json!({ "v": 1, "client_id": "99" })
        );
    }

    #[test]
    fn follow_sends_on_a_change_only() {
        let (tx, rx) = mpsc::channel();
        let mut discord = Discord::with_sender(Some(tx));
        let l = en();
        let none = || Play::default();
        discord.follow(true, Scene::Menu, none, &l);
        discord.follow(true, Scene::Menu, none, &l);
        assert!(matches!(rx.try_recv(), Ok(Msg::Set(Some(_)))));
        assert!(rx.try_recv().is_err());
        // The beatmap is asked for only when a play is shown.
        discord.follow(true, Scene::SongSelect, || unreachable!(), &l);
        assert!(matches!(rx.try_recv(), Ok(Msg::Set(Some(_)))));
        discord.follow(true, Scene::Playing(7), play, &l);
        let Ok(Msg::Set(Some(playing))) = rx.try_recv() else {
            panic!("no status");
        };
        discord.follow(true, Scene::Paused(7), play, &l);
        let Ok(Msg::Set(Some(paused))) = rx.try_recv() else {
            panic!("no status");
        };
        assert!(playing.start.is_some() && paused.start.is_none());
        // Resuming keeps the start of the play.
        discord.follow(true, Scene::Playing(7), play, &l);
        let Ok(Msg::Set(Some(resumed))) = rx.try_recv() else {
            panic!("no status");
        };
        assert_eq!(resumed.start, playing.start);
        // Turning it off clears, once.
        discord.follow(false, Scene::Playing(7), play, &l);
        discord.follow(false, Scene::Menu, play, &l);
        assert!(matches!(rx.try_recv(), Ok(Msg::Set(None))));
        assert!(rx.try_recv().is_err());
        drop(discord);
        assert!(matches!(rx.try_recv(), Ok(Msg::Stop)));
    }

    #[test]
    fn without_an_application_id_nothing_runs() {
        let mut discord = Discord::with_sender(None);
        discord.follow(true, Scene::Menu, Play::default, &en());
        assert_eq!(discord.sent, None);
    }
}
