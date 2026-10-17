//! Global actions of the gameplay screen: lazer's `GlobalAction` with its default bindings.
//!
//! Bindings match as lazer's `GlobalActionContainer` does (`KeyCombinationMatchingMode.Modifiers`):
//! the modifiers held must be exactly those of the binding; other keys may be held too.

use slam_input::{InputEvent, InputKind, MouseButton};

// SDL scancodes (USB HID usage ids).
const SCANCODE_R: u16 = 21;
const SCANCODE_RETURN: u16 = 40;
const SCANCODE_ESCAPE: u16 = 41;
const SCANCODE_SPACE: u16 = 44;
const SCANCODE_MINUS: u16 = 45;
/// `=`, lazer's `Plus` key.
const SCANCODE_EQUALS: u16 = 46;
const SCANCODE_GRAVE: u16 = 53;
const SCANCODE_F10: u16 = 67;
const SCANCODE_DOWN: u16 = 81;
const SCANCODE_UP: u16 = 82;
const SCANCODE_KP_ENTER: u16 = 88;
/// Left control; the eight modifiers follow: shift, alt, GUI, then the right ones.
const SCANCODE_LCTRL: u16 = 224;
const SCANCODE_RGUI: u16 = 231;

/// Modifier kinds, a bit each.
const CTRL: u8 = 1;
const SHIFT: u8 = 2;
const ALT: u8 = 4;
const SUPER: u8 = 8;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Configuration/OsuConfigManager.cs (UIHoldActivationDelay)
/// How long quick retry and quick exit are held to fire, in nanoseconds: the default
/// hold-to-confirm delay, 200 ms.
pub const HOLD_NS: u64 = 200_000_000;

/// A global action of the gameplay screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalAction {
    /// Escape or the first extra mouse button.
    Back,
    /// The middle mouse button.
    PauseGameplay,
    /// Space or the second extra mouse button.
    SkipCutscene,
    /// Held: `` ` `` or Ctrl+R.
    QuickRetry,
    /// Held: Ctrl+`` ` ``.
    QuickExit,
    /// F10: turns the mouse buttons off or on in gameplay (lazer's `MouseDisableButtons`).
    ToggleGameplayMouseButtons,
    /// `=` (the main row's plus): the beatmap offset goes up by a step.
    IncreaseOffset,
    /// `-`: the beatmap offset goes down by a step.
    DecreaseOffset,
}

/// A global action the pause and fail menus take ([`crate::menu_overlay`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    /// Up.
    SelectPrevious,
    /// Down.
    SelectNext,
    /// Space, Enter or the keypad's Enter.
    Select,
}

/// A hold-to-confirm action (lazer's `HoldToConfirmContainer`).
#[derive(Debug, Clone, Copy, Default)]
struct Hold {
    /// Engine time the press started; none while not pressed.
    since: Option<u64>,
    /// The key that pressed it; releasing it, or a modifier, aborts.
    key: u16,
    /// Fired since the press: it fires again only after a release.
    fired: bool,
}

impl Hold {
    fn press(&mut self, key: u16, time_ns: u64) {
        if self.since.is_none() {
            *self = Hold {
                since: Some(time_ns),
                key,
                fired: false,
            };
        }
    }

    fn abort(&mut self) {
        *self = Hold::default();
    }

    fn fire(&mut self, now_ns: u64) -> bool {
        match self.since {
            Some(since) if !self.fired && now_ns.saturating_sub(since) >= HOLD_NS => {
                self.fired = true;
                true
            }
            _ => false,
        }
    }

    fn progress(&self, now_ns: u64) -> f32 {
        match self.since {
            Some(_) if self.fired => 1.0,
            Some(since) => (now_ns.saturating_sub(since) as f64 / HOLD_NS as f64).min(1.0) as f32,
            None => 0.0,
        }
    }
}

/// Recognises global actions in input events. Never allocates.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hotkeys {
    /// Modifier keys held, a bit per scancode from left control.
    modifiers: u8,
    retry: Hold,
    exit: Hold,
}

impl Hotkeys {
    /// Takes one event. Returns the action it presses, if any; quick retry and quick exit
    /// only start their hold here and fire from [`fire`](Self::fire).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Input/Bindings/GlobalActionContainer.cs (globalKeyBindings, InGameKeyBindings)
    pub fn on_event(&mut self, event: &InputEvent) -> Option<GlobalAction> {
        match event.kind {
            InputKind::Key { scancode, down } if is_modifier(scancode) => {
                let bit = 1 << (scancode - SCANCODE_LCTRL);
                if down {
                    self.modifiers |= bit;
                } else {
                    self.modifiers &= !bit;
                }
                // A combination stops matching when one of its keys is released; a
                // modifier pressed while holding changes nothing until the next press.
                if !down {
                    self.retry.abort();
                    self.exit.abort();
                }
                None
            }
            InputKind::Key {
                scancode,
                down: false,
            } => {
                if self.retry.key == scancode {
                    self.retry.abort();
                }
                if self.exit.key == scancode {
                    self.exit.abort();
                }
                None
            }
            InputKind::Key {
                scancode,
                down: true,
            } => {
                let modifiers = self.kinds();
                match (scancode, modifiers) {
                    (SCANCODE_ESCAPE, 0) => Some(GlobalAction::Back),
                    (SCANCODE_SPACE, 0) => Some(GlobalAction::SkipCutscene),
                    (SCANCODE_F10, 0) => Some(GlobalAction::ToggleGameplayMouseButtons),
                    (SCANCODE_EQUALS, 0) => Some(GlobalAction::IncreaseOffset),
                    (SCANCODE_MINUS, 0) => Some(GlobalAction::DecreaseOffset),
                    (SCANCODE_GRAVE, 0) | (SCANCODE_R, CTRL) => {
                        self.retry.press(scancode, event.time_ns);
                        None
                    }
                    (SCANCODE_GRAVE, CTRL) => {
                        self.exit.press(scancode, event.time_ns);
                        None
                    }
                    _ => None,
                }
            }
            InputKind::MouseButton { button, down: true } if self.kinds() == 0 => match button {
                MouseButton::X1 => Some(GlobalAction::Back),
                MouseButton::Middle => Some(GlobalAction::PauseGameplay),
                MouseButton::X2 => Some(GlobalAction::SkipCutscene),
                _ => None,
            },
            _ => None,
        }
    }

    /// The menu action `event` presses, if any. Call it after [`on_event`](Self::on_event)
    /// took the event (for the modifiers held). Space is also
    /// [`GlobalAction::SkipCutscene`]: the menu takes it first while it shows.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Input/Bindings/GlobalActionContainer.cs (globalKeyBindings)
    pub fn menu_key(&self, event: &InputEvent) -> Option<MenuKey> {
        match event.kind {
            InputKind::Key {
                scancode,
                down: true,
            } if self.kinds() == 0 => match scancode {
                SCANCODE_UP => Some(MenuKey::SelectPrevious),
                SCANCODE_DOWN => Some(MenuKey::SelectNext),
                SCANCODE_SPACE | SCANCODE_RETURN | SCANCODE_KP_ENTER => Some(MenuKey::Select),
                _ => None,
            },
            _ => None,
        }
    }

    /// The hold action that fires at engine time `now_ns`, once per press.
    pub fn fire(&mut self, now_ns: u64) -> Option<GlobalAction> {
        if self.retry.fire(now_ns) {
            Some(GlobalAction::QuickRetry)
        } else if self.exit.fire(now_ns) {
            Some(GlobalAction::QuickExit)
        } else {
            None
        }
    }

    /// How far quick retry is held at `now_ns`, 0..=1.
    pub fn retry_progress(&self, now_ns: u64) -> f32 {
        self.retry.progress(now_ns)
    }

    /// How far quick exit is held at `now_ns`, 0..=1.
    pub fn exit_progress(&self, now_ns: u64) -> f32 {
        self.exit.progress(now_ns)
    }

    /// The modifier kinds held.
    fn kinds(&self) -> u8 {
        let both = self.modifiers | (self.modifiers >> 4);
        let mut kinds = 0;
        for (bit, kind) in [(1, CTRL), (2, SHIFT), (4, ALT), (8, SUPER)] {
            if both & bit != 0 {
                kinds |= kind;
            }
        }
        kinds
    }
}

fn is_modifier(scancode: u16) -> bool {
    (SCANCODE_LCTRL..=SCANCODE_RGUI).contains(&scancode)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn key(t: u64, scancode: u16, down: bool) -> InputEvent {
        InputEvent {
            time_ns: t * MS,
            kind: InputKind::Key { scancode, down },
        }
    }

    fn button(t: u64, button: MouseButton) -> InputEvent {
        InputEvent {
            time_ns: t * MS,
            kind: InputKind::MouseButton { button, down: true },
        }
    }

    #[test]
    fn single_presses_map_to_their_actions() {
        let mut h = Hotkeys::default();
        assert_eq!(
            h.on_event(&key(0, SCANCODE_ESCAPE, true)),
            Some(GlobalAction::Back)
        );
        assert_eq!(
            h.on_event(&button(0, MouseButton::X1)),
            Some(GlobalAction::Back)
        );
        assert_eq!(
            h.on_event(&button(0, MouseButton::Middle)),
            Some(GlobalAction::PauseGameplay)
        );
        assert_eq!(
            h.on_event(&key(0, SCANCODE_SPACE, true)),
            Some(GlobalAction::SkipCutscene)
        );
        assert_eq!(
            h.on_event(&button(0, MouseButton::X2)),
            Some(GlobalAction::SkipCutscene)
        );
        assert_eq!(
            h.on_event(&key(0, SCANCODE_F10, true)),
            Some(GlobalAction::ToggleGameplayMouseButtons)
        );
        assert_eq!(
            h.on_event(&key(0, SCANCODE_EQUALS, true)),
            Some(GlobalAction::IncreaseOffset)
        );
        assert_eq!(
            h.on_event(&key(0, SCANCODE_MINUS, true)),
            Some(GlobalAction::DecreaseOffset)
        );
        assert_eq!(h.on_event(&key(0, SCANCODE_ESCAPE, false)), None);
        assert_eq!(h.on_event(&button(0, MouseButton::Left)), None);
    }

    #[test]
    fn menu_keys_need_no_modifiers() {
        let mut h = Hotkeys::default();
        for (scancode, menu_key) in [
            (SCANCODE_UP, MenuKey::SelectPrevious),
            (SCANCODE_DOWN, MenuKey::SelectNext),
            (SCANCODE_SPACE, MenuKey::Select),
            (SCANCODE_RETURN, MenuKey::Select),
            (SCANCODE_KP_ENTER, MenuKey::Select),
        ] {
            let event = key(0, scancode, true);
            h.on_event(&event);
            assert_eq!(h.menu_key(&event), Some(menu_key), "{scancode}");
            assert_eq!(h.menu_key(&key(0, scancode, false)), None);
        }
        assert_eq!(h.menu_key(&key(0, SCANCODE_ESCAPE, true)), None);
        h.on_event(&key(1, SCANCODE_LCTRL + 1, true));
        assert_eq!(h.menu_key(&key(1, SCANCODE_RETURN, true)), None);
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let mut h = Hotkeys::default();
        h.on_event(&key(0, SCANCODE_LCTRL, true));
        assert_eq!(h.on_event(&key(1, SCANCODE_ESCAPE, true)), None);
        assert_eq!(h.on_event(&button(1, MouseButton::Middle)), None);
        // Ctrl+` is quick exit, not quick retry.
        h.on_event(&key(2, SCANCODE_GRAVE, true));
        assert_eq!(h.fire(202 * MS), Some(GlobalAction::QuickExit));
    }

    #[test]
    fn quick_retry_fires_once_after_a_200_ms_hold() {
        let mut h = Hotkeys::default();
        h.on_event(&key(100, SCANCODE_GRAVE, true));
        assert_eq!(h.fire(299 * MS), None);
        assert!((h.retry_progress(200 * MS) - 0.5).abs() < 1e-6);
        assert_eq!(h.fire(300 * MS), Some(GlobalAction::QuickRetry));
        assert_eq!(h.fire(400 * MS), None);
        h.on_event(&key(500, SCANCODE_GRAVE, false));
        assert_eq!(h.retry_progress(500 * MS), 0.0);
        // Right control works as control.
        h.on_event(&key(600, SCANCODE_LCTRL + 4, true));
        h.on_event(&key(600, SCANCODE_R, true));
        assert_eq!(h.fire(800 * MS), Some(GlobalAction::QuickRetry));
    }

    #[test]
    fn releasing_early_aborts_the_hold() {
        let mut h = Hotkeys::default();
        h.on_event(&key(0, SCANCODE_LCTRL, true));
        h.on_event(&key(0, SCANCODE_R, true));
        h.on_event(&key(150, SCANCODE_LCTRL, false));
        assert_eq!(h.fire(300 * MS), None);
        h.on_event(&key(400, SCANCODE_GRAVE, true));
        h.on_event(&key(500, SCANCODE_GRAVE, false));
        assert_eq!(h.fire(700 * MS), None);
    }
}
