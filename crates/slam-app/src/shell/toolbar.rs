//! The shell's side of the menus' toolbar (ADR-0168): input and the popup's settings, the
//! pinned rows, the clock. The toolbar's state and drawing are `slam_ui::toolbar`.
//!
//! A popup edits a copy of the settings, as the settings panel does, and commits it when it
//! closes: live settings follow the popup's changes, and the file is saved once.

use slam_ui::settings_screen::RowValue;
use slam_ui::toolbar::{MAX_PINS, ToolbarEvent, ToolbarFrame, ToolbarLayout};

use super::*;
use crate::menu_sounds::MenuSound;

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

    /// Whether the toolbar shows: on the menus, not in the play and not over the intro.
    pub(super) fn toolbar_shown(&self) -> bool {
        !matches!(self.stack.top(), AppScreen::Play(_)) && self.intro.is_none()
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
        if !self.toolbar_shown() || self.settings_open() {
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
        let shown = self.toolbar_shown();
        if !shown && (self.toolbar.popup_open() || self.popup_editing) {
            self.close_popup();
        }
        if self.settings_open() {
            // The panel covers the bar: nothing under the pointer.
            let layout = self.toolbar_layout();
            self.toolbar.pointer(-1.0, -1.0, &layout);
        }
        if now_ns >= self.clock_at_ns {
            self.clock = crate::local_time::minute_now();
            self.clock_at_ns = now_ns.saturating_add(1_000_000_000);
        }
        let frame: ToolbarFrame = self.toolbar.frame(shown, self.clock);
        snapshot.toolbar = frame;
    }
}

#[cfg(test)]
mod tests;
