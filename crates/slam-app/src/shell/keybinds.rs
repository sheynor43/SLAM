//! The key binding rows of the settings screen: converting inputs, the labels of keys, and
//! what the screen's key actions do to the edited settings (ADR-0049).

use slam_input::MouseButton;
use slam_play::input::Input;
use slam_ui::i18n::{FluentArgs, Localizer};
use slam_ui::settings_screen::{KeyLabels, KeyValue, MouseKey, SettingsAction, SettingsScreen};

use super::rows::Row;
use crate::settings::{BINDING_SLOTS, Settings};

/// How many scancodes get a label: SDL's scancodes are below 512.
const SCANCODES: usize = 512;

/// The screen's value for a binding slot.
pub fn key_value(input: Option<Input>) -> KeyValue {
    match input {
        None => KeyValue::Empty,
        Some(Input::Key(code)) => KeyValue::Key(code),
        Some(Input::Mouse(button)) => KeyValue::Mouse(match button {
            MouseButton::Left => MouseKey::Left,
            MouseButton::Middle => MouseKey::Middle,
            MouseButton::Right => MouseKey::Right,
            MouseButton::X1 => MouseKey::X1,
            MouseButton::X2 => MouseKey::X2,
        }),
    }
}

/// The input a captured value stands for; `None` for an empty slot.
pub fn input_of(value: KeyValue) -> Option<Input> {
    match value {
        KeyValue::Empty => None,
        KeyValue::Key(code) => Some(Input::Key(code)),
        KeyValue::Mouse(button) => Some(Input::Mouse(match button {
            MouseKey::Left => MouseButton::Left,
            MouseKey::Middle => MouseButton::Middle,
            MouseKey::Right => MouseButton::Right,
            MouseKey::X1 => MouseButton::X1,
            MouseKey::X2 => MouseButton::X2,
        })),
    }
}

/// The Fluent key of a named keyboard key.
fn named_key(scancode: u16) -> Option<&'static str> {
    Some(match scancode {
        40 => "key-enter",
        41 => "key-escape",
        42 => "key-backspace",
        43 => "key-tab",
        44 => "key-space",
        45 => "key-minus",
        46 => "key-equals",
        47 => "key-left-bracket",
        48 => "key-right-bracket",
        49 => "key-backslash",
        51 => "key-semicolon",
        52 => "key-apostrophe",
        53 => "key-grave",
        54 => "key-comma",
        55 => "key-period",
        56 => "key-slash",
        57 => "key-caps-lock",
        79 => "key-right",
        80 => "key-left",
        81 => "key-down",
        82 => "key-up",
        224 => "key-left-ctrl",
        225 => "key-left-shift",
        226 => "key-left-alt",
        228 => "key-right-ctrl",
        229 => "key-right-shift",
        230 => "key-right-alt",
        _ => return None,
    })
}

/// The texts of the key rows and of the conflict question, formatted once when the screen
/// is built: letters, digits and function keys are shown as they are, the other named keys
/// and the mouse buttons come from Fluent, and so does the "Key 123" of a scancode without a
/// name.
pub fn key_labels(localizer: &Localizer) -> KeyLabels {
    let text = |key: &str| localizer.format(key, None).into_owned();
    let keys = (0..SCANCODES as u16)
        .map(|code| match code {
            4..=39 | 58..=69 => crate::settings::keys::key_name(code),
            _ => match named_key(code) {
                Some(key) => text(key),
                None => {
                    let mut args = FluentArgs::new();
                    args.set("code", code.to_string());
                    localizer.format("key-unknown", Some(&args)).into_owned()
                }
            },
        })
        .collect();
    KeyLabels {
        empty: text("settings-keybind-empty"),
        prompt: text("settings-keybind-listening"),
        keys,
        mouse: [
            text("key-mouse-left"),
            text("key-mouse-middle"),
            text("key-mouse-right"),
            text("key-mouse-x1"),
            text("key-mouse-x2"),
        ],
        conflict_title: text("settings-keybind-conflict-title"),
        conflict_keep: text("settings-keybind-conflict-keep"),
        conflict_apply: text("settings-keybind-conflict-apply"),
    }
}

/// Shows the bindings of `settings` in every key row of `screen`.
pub fn refresh(screen: &mut SettingsScreen, settings: &Settings) {
    for (row, kind) in Row::ALL.iter().enumerate() {
        if let Some((action, slot)) = kind.key_slot() {
            screen.set_key(row, key_value(settings.input.slot(action, slot)));
        }
    }
}

/// Does what the screen asked about key bindings to the edited `settings`, and shows the
/// result in the screen. Other actions are not its business.
pub fn apply(screen: &mut SettingsScreen, settings: &mut Settings, action: SettingsAction) {
    match action {
        SettingsAction::KeyCaptured { row, key } => {
            let (Some((action, slot)), Some(input)) =
                (Row::ALL.get(row).and_then(|r| r.key_slot()), input_of(key))
            else {
                return;
            };
            match settings.input.conflict(action, slot, input) {
                None => {
                    settings.input.bind(action, slot, input);
                }
                Some((other, other_slot)) => {
                    let other_row = if other_slot < BINDING_SLOTS {
                        Row::key_row(other, other_slot)
                    } else {
                        None
                    };
                    screen.ask_conflict(row, key, other_row);
                }
            }
        }
        SettingsAction::ConflictResolved {
            row,
            key,
            apply: true,
        } => {
            if let (Some((action, slot)), Some(input)) =
                (Row::ALL.get(row).and_then(|r| r.key_slot()), input_of(key))
            {
                settings.input.bind(action, slot, input);
            }
        }
        SettingsAction::KeyCleared { row } => {
            if let Some((action, slot)) = Row::ALL.get(row).and_then(|r| r.key_slot()) {
                settings.input.clear(action, slot);
            }
        }
        SettingsAction::Pressed { row } => {
            if Row::ALL.get(row) == Some(&Row::ResetKeys) {
                settings.input.reset();
            }
        }
        _ => return,
    }
    refresh(screen, settings);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_convert_both_ways() {
        let inputs = [
            Input::Key(29),
            Input::Key(300),
            Input::Mouse(MouseButton::Left),
            Input::Mouse(MouseButton::Middle),
            Input::Mouse(MouseButton::Right),
            Input::Mouse(MouseButton::X1),
            Input::Mouse(MouseButton::X2),
        ];
        for input in inputs {
            assert_eq!(input_of(key_value(Some(input))), Some(input));
        }
        assert_eq!(key_value(None), KeyValue::Empty);
        assert_eq!(input_of(KeyValue::Empty), None);
    }

    #[test]
    fn every_key_and_mouse_label_exists_in_both_languages() {
        let mut localizer = Localizer::builtin().unwrap();
        for language in ["en", "ru"] {
            localizer.set_language(language).unwrap();
            let labels = key_labels(&localizer);
            assert_eq!(labels.keys.len(), SCANCODES);
            let texts = labels.keys.iter().chain(&labels.mouse).chain([
                &labels.empty,
                &labels.prompt,
                &labels.conflict_title,
                &labels.conflict_keep,
                &labels.conflict_apply,
            ]);
            for text in texts {
                // An unknown Fluent key formats as itself.
                assert!(!text.is_empty());
                assert!(
                    !text.starts_with("key-") && !text.starts_with("settings-"),
                    "{language}: {text}"
                );
            }
            // Fluent adds no direction marks around the number.
            assert!(labels.keys[123].contains("123") && !labels.keys[123].contains('\u{2068}'));
        }
        localizer.set_language("en").unwrap();
        let labels = key_labels(&localizer);
        assert_eq!(labels.keys[29], "Z");
        assert_eq!(labels.keys[39], "0");
        assert_eq!(labels.keys[58], "F1");
        assert_eq!(labels.keys[41], "Escape");
        assert_eq!(labels.keys[49], "\\");
        assert_eq!(labels.keys[123], "Key 123");
        assert_eq!(labels.mouse[0], "Mouse left");
    }
}
