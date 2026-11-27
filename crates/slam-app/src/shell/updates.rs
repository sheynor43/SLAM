//! The update tick's side of the self-updater (ADR-0185): the state shown on the settings
//! panel's rows and in the toasts. The thread (`crate::update`) does the network and file
//! work; this only sends it commands and polls what it tells.

use slam_ui::i18n::{FluentArgs, Localizer};
use slam_ui::settings_screen::SettingsScreen;

use super::rows::Row;
use crate::update::{Channel, Command, Event, Failure, State, Updater};

/// The toast variants of the updater, after the import and pin ones (`import::toast_keys`).
pub const TOAST_AVAILABLE: u8 = 9;
pub const TOAST_READY: u8 = 10;

/// What a poll found out.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Polled {
    /// The state changed: the rows need to be shown again.
    pub changed: bool,
    /// A toast to show.
    pub toast: Option<u8>,
    /// The files are in place: the application restarts.
    pub installed: bool,
}

pub struct UpdatesSection {
    updater: Option<Updater>,
    state: State,
    /// The start-up check was sent.
    started: bool,
    /// The check in flight is the start-up one: a version it finds is told by a toast.
    toast_on_found: bool,
}

impl UpdatesSection {
    pub fn new(updater: Option<Updater>) -> UpdatesSection {
        let state = if crate::update::available() {
            State::Idle
        } else {
            State::Unavailable
        };
        UpdatesSection {
            updater,
            state,
            started: false,
            toast_on_found: false,
        }
    }

    /// Sends the start-up check once, unless the player turned it off.
    pub fn start(&mut self, enabled: bool, channel: Channel) {
        if self.started {
            return;
        }
        self.started = true;
        if enabled && crate::update::available() {
            self.toast_on_found = true;
            self.send(Command::Check {
                channel,
                manual: false,
            });
        }
    }

    /// The player asked for a check.
    pub fn check(&mut self, channel: Channel) {
        self.toast_on_found = false;
        self.send(Command::Check {
            channel,
            manual: true,
        });
    }

    /// The player said "Update".
    pub fn download(&self) {
        self.send(Command::Download);
    }

    /// The player said "Restart".
    pub fn install(&self) {
        self.send(Command::Install);
    }

    /// What a click on the update toast does: starts the download of an available version,
    /// installs a downloaded one. Returns whether only the release page can be offered.
    pub fn answer_toast(&self, variant: u8) -> bool {
        match (variant, &self.state) {
            (TOAST_AVAILABLE, State::Available(a)) if a.installable => self.download(),
            (TOAST_AVAILABLE, State::Available(_)) => return true,
            (TOAST_READY, State::Ready { .. }) => self.install(),
            _ => {}
        }
        false
    }

    fn send(&self, command: Command) {
        if let Some(updater) = &self.updater {
            updater.send(command);
        }
    }

    /// Takes what the thread told since the last tick. Does not allocate when it told
    /// nothing.
    pub fn poll(&mut self) -> Polled {
        let mut polled = Polled::default();
        while let Some(event) = self.updater.as_ref().and_then(Updater::poll) {
            match event {
                Event::State(state) => {
                    polled.changed = true;
                    match &state {
                        State::Available(_) if std::mem::take(&mut self.toast_on_found) => {
                            polled.toast = Some(TOAST_AVAILABLE);
                        }
                        State::Ready { .. } => polled.toast = Some(TOAST_READY),
                        State::UpToDate | State::Idle | State::Failed(_) => {
                            self.toast_on_found = false;
                        }
                        _ => {}
                    }
                    self.state = state;
                }
                Event::Installed => polled.installed = true,
            }
        }
        polled
    }

    /// The status line's text in the language of `localizer`.
    pub fn status_text(&self, localizer: &Localizer) -> String {
        let text = |key: &str| localizer.format(key, None).into_owned();
        let with = |key: &str, name: &str, value: &str| {
            let mut args = FluentArgs::new();
            args.set(name, value);
            localizer.format(key, Some(&args)).into_owned()
        };
        match &self.state {
            State::Idle => text("settings-updates-status"),
            State::Unavailable => text("settings-updates-status-unavailable"),
            State::Checking => text("settings-updates-status-checking"),
            State::UpToDate => text("settings-updates-status-up-to-date"),
            State::Available(a) => with("settings-updates-status-available", "version", &a.version),
            State::Downloading(percent) => with(
                "settings-updates-status-downloading",
                "percent",
                &percent.to_string(),
            ),
            State::Ready { version } => with("settings-updates-status-ready", "version", version),
            State::Failed(Failure::Network) => text("settings-updates-status-failed-network"),
            State::Failed(Failure::Signature) => text("settings-updates-status-failed-signature"),
            State::Failed(Failure::Install) => text("settings-updates-status-failed-install"),
        }
    }

    /// Shows which of the rows apply to the state.
    pub fn refresh(&self, screen: &mut SettingsScreen) {
        let (installable, page, ready, busy) = match &self.state {
            State::Available(a) => (a.installable, !a.installable, false, false),
            State::Ready { .. } => (false, false, true, false),
            State::Checking | State::Downloading(_) => (false, false, false, true),
            _ => (false, false, false, false),
        };
        let index = |row: Row| Row::ALL.iter().position(|r| *r == row);
        let mut set = |row: Row, shown: bool| {
            if let Some(i) = index(row) {
                screen.set_hidden(i, !shown);
            }
        };
        set(Row::UpdatesInstall, installable);
        set(Row::UpdatesPage, page);
        set(Row::UpdatesRestart, ready);
        let off = busy || self.updater.is_none() || matches!(self.state, State::Unavailable);
        if let Some(i) = index(Row::UpdatesStatus) {
            screen.set_disabled(i, off);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::Available;

    fn section(state: State) -> UpdatesSection {
        UpdatesSection {
            updater: None,
            state,
            started: false,
            toast_on_found: false,
        }
    }

    fn found() -> State {
        State::Available(Available {
            version: "0.2.0-alpha".into(),
            page_url: String::new(),
            installable: true,
        })
    }

    #[test]
    fn a_click_on_the_toast_downloads_installs_or_offers_the_page() {
        assert!(!section(found()).answer_toast(TOAST_AVAILABLE));
        let page = State::Available(Available {
            installable: false,
            ..match found() {
                State::Available(a) => a,
                _ => unreachable!(),
            }
        });
        assert!(section(page).answer_toast(TOAST_AVAILABLE));
        assert!(!section(State::Idle).answer_toast(TOAST_AVAILABLE));
    }

    #[test]
    fn status_lines_cover_every_state_in_both_languages() {
        let states = [
            State::Idle,
            State::Unavailable,
            State::Checking,
            State::UpToDate,
            found(),
            State::Downloading(42),
            State::Ready {
                version: "0.2.0-alpha".into(),
            },
            State::Failed(Failure::Network),
            State::Failed(Failure::Signature),
            State::Failed(Failure::Install),
        ];
        let localizer = Localizer::builtin().unwrap();
        for language in ["en", "ru"] {
            localizer.set_language(language).unwrap();
            for state in &states {
                let text = section(state.clone()).status_text(&localizer);
                assert!(
                    !text.is_empty() && !text.contains("settings-updates"),
                    "{text}"
                );
                // The arguments are filled in, without Fluent's isolation marks mixed up.
                assert!(!text.contains("{ $"), "{text}");
            }
        }
        localizer.set_language("en").unwrap();
        assert_eq!(
            section(State::Downloading(42))
                .status_text(&localizer)
                .replace(['\u{2068}', '\u{2069}'], ""),
            "Downloading the update: 42%"
        );
    }

    #[test]
    fn a_section_without_a_thread_polls_nothing_and_keeps_its_state() {
        let mut s = section(State::UpToDate);
        assert_eq!(s.poll(), Polled::default());
        assert_eq!(s.state, State::UpToDate);
    }
}
