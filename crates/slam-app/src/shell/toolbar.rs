//! The shell's side of the menus' toolbar (ADR-0168): input and the popup's settings, the
//! pinned rows, the clock. The toolbar's state and drawing are `slam_ui::toolbar`.
//!
//! A popup edits a copy of the settings, as the settings panel does, and commits it when it
//! closes: live settings follow the popup's changes, and the file is saved once.

use slam_ui::settings_screen::RowValue;
use slam_ui::toolbar::{
    MAX_PINS, NowPlayingFrame, NowPlayingText, PlayerAction, ToolbarEvent, ToolbarFrame,
    ToolbarLayout,
};

use super::*;
use crate::menu_sounds::MenuSound;

/// What the toolbar's music player asked of the menu music since the shell last took it
/// (ADR-0178). A seek keeps the latest position of a drag.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MusicCommands {
    pub previous: bool,
    pub play_pause: bool,
    pub next: bool,
    /// Share of the track to seek to, 0 ..= 1.
    pub seek: Option<f32>,
}

impl ShellState {
    /// Where the toolbar's parts are in the window.
    fn toolbar_layout(&self) -> ToolbarLayout {
        ToolbarLayout::new(
            self.size[0],
            self.size[1],
            &self.theme,
            self.toolbar.pins().len(),
        )
    }

    /// Whether the toolbar is wanted in view: on the song select and the main menu, not in
    /// the play and not on the results (ADR-0178). Over the intro it comes in when the
    /// black starts to fade, with the menu, so it is there as the menu is.
    pub(super) fn toolbar_wanted(&self, now_ns: u64) -> bool {
        let screen = !matches!(
            self.stack.top(),
            AppScreen::Play(_) | AppScreen::Results(_) | AppScreen::Loader(_)
        );
        let revealed = match &self.intro {
            Some(intro) => intro.reveal_at().is_some_and(|at| now_ns >= at),
            None => true,
        };
        screen && revealed
    }

    /// How far in the toolbar is while a screen transition runs: the opacity of the layers
    /// that show it (not a play or the results), so it fades and slides with them. `None`
    /// at rest, without a transition, and over the intro.
    pub(super) fn toolbar_transition(&self, now_ns: u64) -> Option<f32> {
        if !self.stack.in_transition(now_ns) || self.intro.is_some() {
            return None;
        }
        let shown: f32 = self
            .stack
            .layers(now_ns)
            .iter()
            .flatten()
            .filter(|(screen, _)| {
                !matches!(
                    screen,
                    AppScreen::Play(_) | AppScreen::Results(_) | AppScreen::Loader(_)
                )
            })
            .map(|(_, pose)| pose.opacity)
            .sum();
        Some(shown.clamp(0.0, 1.0))
    }

    /// What the music player asked for since the last call.
    pub fn take_music_commands(&mut self) -> MusicCommands {
        std::mem::take(&mut self.music_commands)
    }

    /// What the menu music is doing, for the player: whether there is a track, whether it
    /// is paused, where it is and how long it is (milliseconds).
    pub fn set_now_playing(
        &mut self,
        available: bool,
        paused: bool,
        position_ms: Option<f64>,
        length_ms: Option<f64>,
    ) {
        self.now_playing = NowPlayingFrame {
            available,
            paused,
            position_ms: position_ms.unwrap_or(0.0) as f32,
            length_ms: length_ms.unwrap_or(0.0) as f32,
            beat: self.now_playing.beat,
            stars: self.now_playing.stars,
        };
    }

    /// The beat of what the menu music plays, 0 to 1, for the player's glow, and its
    /// difficulty's star rating, for the outline; `None` when unknown.
    pub fn set_now_playing_beat(&mut self, beat: Option<f32>, stars: Option<f32>) {
        self.now_playing.beat = beat;
        self.now_playing.stars = stars;
    }

    /// Follows the track the menu music plays: the player shows its set's title and artist.
    /// Allocates only when the set changes.
    fn refresh_now_text(&mut self) {
        let set = self
            .menu_track
            .and_then(|(set, _)| self.song_select.list().sets.get(set));
        let key = set.map(|set| set.key);
        if key == self.now_key {
            return;
        }
        self.now_key = key;
        self.now_text = match set {
            Some(set) => NowPlayingText {
                key: set.key,
                title: set.title.as_str().into(),
                artist: set.artist.as_str().into(),
            },
            None => NowPlayingText::default(),
        };
    }

    /// A press in the player's popup. On the song select Previous and Next move the
    /// selection to the neighbouring set (the music follows it); elsewhere they go to the
    /// menu music with the rest.
    fn player_action(&mut self, action: PlayerAction) {
        let on_song_select = matches!(self.stack.top(), AppScreen::SongSelect);
        match action {
            PlayerAction::Previous | PlayerAction::Next if on_song_select => {
                let input = if action == PlayerAction::Next {
                    SongSelectInput::Right
                } else {
                    SongSelectInput::Left
                };
                let _ = self.song_select.input(input, &self.song_layout);
                self.ui_sounds.push(MenuSound::Click);
            }
            PlayerAction::Previous => {
                self.music_commands.previous = true;
                self.ui_sounds.push(MenuSound::Click);
            }
            PlayerAction::Next => {
                self.music_commands.next = true;
                self.ui_sounds.push(MenuSound::Click);
            }
            PlayerAction::PlayPause => {
                self.music_commands.play_pause = !self.music_commands.play_pause;
                self.ui_sounds.push(MenuSound::Click);
            }
            PlayerAction::Seek(share) => self.music_commands.seek = Some(share),
        }
    }

    /// The rows the settings in force pin, as indices on the settings screen, in the order
    /// pinned; names no row has are skipped. Does not allocate.
    fn pinned_rows(&self) -> ([u16; MAX_PINS], usize) {
        let mut rows = [0_u16; MAX_PINS];
        let mut count = 0;
        for name in &self.live_settings().general.pinned_settings {
            let at = Row::ALL.iter().position(|row| row.name() == name);
            if let Some(at) = at
                && count < MAX_PINS
            {
                rows[count] = at as u16;
                count += 1;
            }
        }
        (rows, count)
    }

    /// Gives the toolbar the pins of the settings in force. A popup of a pin that changed
    /// closes.
    pub(super) fn refresh_pins(&mut self) {
        let (rows, count) = self.pinned_rows();
        if self.toolbar.set_pins(&rows[..count]) {
            self.close_popup();
        }
    }

    /// The toolbar's part of the input, before the screens'. Returns whether it took the
    /// event: a press on the bar or its popup, Escape with a popup open, the wheel over its
    /// dropdown list.
    pub(super) fn toolbar_input(&mut self, kind: InputKind, now_ns: u64) -> bool {
        if !self.toolbar_wanted(now_ns) || self.intro.is_some() || self.settings_open() {
            return false;
        }
        let layout = self.toolbar_layout();
        let event = match kind {
            InputKind::MouseMove { x, y } => {
                self.cursor = [x * self.pixels_per_point, y * self.pixels_per_point];
                let [x, y] = self.cursor;
                // A slider's drag goes on; the screens under the bar see the move too.
                if let Some(event) = self.toolbar.pointer(x, y, &layout) {
                    self.toolbar_event(event, now_ns);
                }
                return false;
            }
            InputKind::MouseButton {
                button: MouseButton::Left,
                down: true,
            } => {
                let [x, y] = self.cursor;
                self.toolbar.press(x, y, &layout)
            }
            InputKind::MouseButton {
                button: MouseButton::Left,
                down: false,
            } => {
                self.toolbar.release(&layout);
                return false;
            }
            InputKind::Key {
                scancode: KEY_ESCAPE,
                down: true,
            } => self.toolbar.back(&layout),
            InputKind::MouseWheel { y, .. } => {
                let [px, py] = self.cursor;
                return self.toolbar.scroll(-y, px, py, &layout);
            }
            // A slider of the popup typed into (ADR-0173): Enter starts and commits, text and
            // Backspace edit.
            InputKind::Key {
                scancode: KEY_ENTER | KEY_KEYPAD_ENTER,
                down: true,
            } => self.toolbar.key(SettingsInput::Activate, &layout),
            InputKind::Key {
                scancode: KEY_BACKSPACE,
                down: true,
            }
            | InputKind::KeyRepeat {
                scancode: KEY_BACKSPACE,
            } => {
                let erase = if self.mods().contains(Mods::CTRL) {
                    SettingsInput::EraseWord
                } else {
                    SettingsInput::Erase
                };
                self.toolbar.key(erase, &layout)
            }
            // Arrows step a popup's slider, held ones repeat.
            InputKind::Key {
                scancode: code @ (KEY_LEFT | KEY_RIGHT),
                down: true,
            }
            | InputKind::KeyRepeat {
                scancode: code @ (KEY_LEFT | KEY_RIGHT),
            } if !self.mods().contains(Mods::ALT) => {
                let step = if code == KEY_LEFT {
                    SettingsInput::Left
                } else {
                    SettingsInput::Right
                };
                self.toolbar.key(step, &layout)
            }
            InputKind::Text(text) if self.toolbar.is_editing() => {
                for c in text.as_str().chars() {
                    self.toolbar.key(SettingsInput::Type(c), &layout);
                }
                Some(ToolbarEvent::Absorbed)
            }
            _ => None,
        };
        match event {
            Some(event) => {
                self.toolbar_event(event, now_ns);
                true
            }
            None => false,
        }
    }

    fn toolbar_event(&mut self, event: ToolbarEvent, now_ns: u64) {
        match event {
            ToolbarEvent::Absorbed => {}
            ToolbarEvent::OpenSettings => {
                self.close_popup();
                if self.settings_allowed() {
                    self.open_settings(now_ns);
                }
            }
            ToolbarEvent::Home => {
                self.close_popup();
                self.go_home(now_ns);
            }
            ToolbarEvent::OpenPopup { pin } => self.open_popup(pin, now_ns),
            ToolbarEvent::ClosePopup => self.close_popup(),
            ToolbarEvent::OpenPlayer => {
                self.close_popup();
                self.toolbar.show_player(now_ns);
            }
            ToolbarEvent::ClosePlayer => {}
            ToolbarEvent::Player(action) => self.player_action(action),
            ToolbarEvent::Changed { row, value } => {
                self.ui_sounds.push(match value {
                    RowValue::Toggle(true) => MenuSound::ToggleOn,
                    RowValue::Toggle(false) => MenuSound::ToggleOff,
                    RowValue::Slider(_) => MenuSound::SliderStep,
                    RowValue::Choice(_) => MenuSound::Click,
                });
                if let Some(editing) = &mut self.editing {
                    self.rows.apply(editing, row, value);
                }
            }
        }
    }

    /// Shows the popup of the pin at `pin` over the settings to edit. Allocates, as opening
    /// the settings panel does.
    fn open_popup(&mut self, pin: usize, now_ns: u64) {
        self.close_popup();
        let Some(row) = self
            .toolbar
            .pins()
            .get(pin)
            .and_then(|at| Row::ALL.get(usize::from(*at)))
        else {
            return;
        };
        let widget = self.rows.pin_widget(*row, &self.settings);
        let Some(widget) = widget else {
            return;
        };
        self.editing = Some(self.settings.clone());
        self.popup_editing = true;
        self.toolbar.show_popup(pin, widget, now_ns);
    }

    /// Closes the popup and commits what it changed.
    pub(super) fn close_popup(&mut self) {
        self.toolbar.close_popup();
        if !std::mem::take(&mut self.popup_editing) {
            return;
        }
        if commit_settings(
            &mut self.settings,
            &mut self.editing,
            &mut self.save_request,
        ) {
            self.settings_changed = true;
        }
    }

    /// Back to the main menu from any menu screen. Not while a play loads.
    fn go_home(&mut self, now_ns: u64) {
        if self.loading {
            return;
        }
        while self.stack.depth() > 1 && !matches!(self.stack.top(), AppScreen::Play(_)) {
            if matches!(self.stack.top(), AppScreen::Results(_)) {
                self.track_stop = true;
            }
            self.stack.set_origin(Some(self.cursor));
            self.stack.navigate(Nav::Pop, now_ns);
        }
    }

    /// The right-click on a row of the open settings panel: pins it to the toolbar or takes
    /// it off, and tells which with the toast. Returns whether it took the event.
    pub(super) fn pin_input(&mut self, kind: InputKind, now_ns: u64) -> bool {
        let InputKind::MouseButton {
            button: MouseButton::Right,
            down: true,
        } = kind
        else {
            return false;
        };
        let Some(panel) = self.panel.as_ref().filter(|p| p.motion.is_open()) else {
            return false;
        };
        // A key row that listens takes the button as its binding.
        if panel.screen.is_listening() {
            return false;
        }
        let [x, y] = self.cursor;
        let width = self.settings_layout.width();
        let layout = self
            .settings_layout
            .shifted(panel.motion.shift(now_ns, width));
        if let Some(row) = panel.screen.row_under(&layout, x, y) {
            self.toggle_pin(row, now_ns);
        }
        true
    }

    /// Pins settings row `row` or, pinned, unpins it, in the edited settings.
    fn toggle_pin(&mut self, row: usize, now_ns: u64) {
        let Some(row) = Row::ALL.get(row).copied() else {
            return;
        };
        let pinnable = self.rows.pin_widget(row, &self.settings).is_some();
        let Some(editing) = &mut self.editing else {
            return;
        };
        let pins = &mut editing.general.pinned_settings;
        let variant = if let Some(at) = pins.iter().position(|name| name == row.name()) {
            pins.remove(at);
            crate::import::TOAST_UNPINNED
        } else if !pinnable {
            return;
        } else if pins.len() >= MAX_PINS {
            crate::import::TOAST_PIN_FULL
        } else {
            pins.push(row.name().to_owned());
            crate::import::TOAST_PINNED
        };
        self.refresh_pins();
        self.toast.show_variant(variant, now_ns);
    }

    /// One tick of the toolbar: closes a popup the screen went from under, keeps the clock,
    /// and writes the frame. Does not allocate.
    pub(super) fn tick_toolbar(&mut self, now_ns: u64, snapshot: &mut ShellSnapshot) {
        let wanted = self.toolbar_wanted(now_ns);
        if !wanted {
            if self.toolbar.popup_open() || self.popup_editing {
                self.close_popup();
            }
            self.toolbar.close_player();
        }
        // While screens change, the bar follows their transition (ADR-0126): it goes with
        // the leaving screen into a play and comes with the arriving one; elsewhere it
        // runs its own slide.
        let followed = self.toolbar_transition(now_ns);
        let slide = match followed {
            Some(value) => self.toolbar_slide.follow(value, wanted),
            None => self.toolbar_slide.update(wanted, now_ns, &self.theme),
        };
        self.refresh_now_text();
        if self.settings_open() {
            // The panel covers the bar: nothing under the pointer.
            let layout = self.toolbar_layout();
            self.toolbar.pointer(-1.0, -1.0, &layout);
        }
        if now_ns >= self.clock_at_ns {
            self.clock = crate::local_time::minute_now();
            self.clock_at_ns = now_ns.saturating_add(1_000_000_000);
        }
        let mut frame: ToolbarFrame = self.toolbar.frame(wanted || slide > 0.0, self.clock);
        frame.slide = slide;
        frame.follows = followed.is_some();
        frame.now_playing = self.now_playing;
        snapshot.toolbar = frame;
        snapshot.now_playing = self.now_text.clone();
    }
}

#[cfg(test)]
mod tests;
